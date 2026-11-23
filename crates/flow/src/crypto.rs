//! Identity and pairing primitives. Nothing here invents cryptography:
//!
//! * the identity is an ECDSA P-256 self-signed X.509 certificate (made with `rcgen`, the
//!   same profile the Python version asked the `openssl` CLI for); peers are pinned by the
//!   SHA-256 of its SubjectPublicKeyInfo;
//! * pairing uses a hash commitment + short authentication string (SAS), the construction
//!   of ZRTP (RFC 6189) / Bluetooth numeric comparison, over a mutually authenticated
//!   TLS 1.3 session.
//!
//! Never log nonces, SAS values or key material.

use std::fs;
use std::io::Write;
use std::path::Path;

use glisa_i18n::n_;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

use crate::text::{Text, plain};
use crate::util::{hex, random_bytes, sha256, unhex};

pub const NONCE_BYTES: usize = 32;
/// Days of validity, counted from today (as `openssl req -days 9125`).
const VALID_DAYS: i64 = 9125;

#[derive(Debug)]
pub struct IdentityError(pub Text);

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.english())
    }
}

impl std::error::Error for IdentityError {}

// ================================================================== identity
/// New P-256 key + self-signed certificate (files 0600, directory 0700). notBefore is in
/// 2020 so a peer whose clock lags still accepts it; 25 years of validity. Extensions as
/// the Python version's openssl configuration: CA:TRUE (critical), keyUsage
/// digitalSignature + keyCertSign (critical), extendedKeyUsage serverAuth + clientAuth,
/// subjectKeyIdentifier.
pub fn generate_identity(cert_path: &Path, key_path: &Path) -> Result<(), IdentityError> {
    let fail = |e: &dyn std::fmt::Display| {
        IdentityError(
            Text::new(n_!("Couldn't create this computer's identity: {error}"))
                .arg("error", e.to_string()),
        )
    };
    let dir = cert_path.parent().unwrap_or(Path::new("."));
    private_dir(dir).map_err(|e| fail(&e))?;
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(|e| fail(&e))?;
    let mut params = rcgen::CertificateParams::default();
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, crate::wire::CERT_COMMON_NAME);
    params.distinguished_name = dn;
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::DigitalSignature,
        rcgen::KeyUsagePurpose::KeyCertSign,
    ];
    params.extended_key_usages = vec![
        rcgen::ExtendedKeyUsagePurpose::ServerAuth,
        rcgen::ExtendedKeyUsagePurpose::ClientAuth,
    ];
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(VALID_DAYS);
    let mut serial = random_bytes::<16>();
    serial[0] &= 0x7F;
    serial[0] |= 0x40; // positive, full length
    params.serial_number = Some(rcgen::SerialNumber::from_slice(&serial));
    params.use_authority_key_identifier_extension = false;
    params.key_identifier_method = rcgen::KeyIdMethod::Sha256;
    let cert = params.self_signed(&key).map_err(|e| fail(&e))?;
    let tmp_key = with_suffix(key_path, ".new");
    let tmp_cert = with_suffix(cert_path, ".new");
    let result = (|| -> std::io::Result<()> {
        write_private(&tmp_key, key.serialize_pem().as_bytes())?;
        write_private(&tmp_cert, cert.pem().as_bytes())?;
        fs::rename(&tmp_key, key_path)?;
        fs::rename(&tmp_cert, cert_path)?;
        Ok(())
    })();
    let _ = fs::remove_file(&tmp_key);
    let _ = fs::remove_file(&tmp_cert);
    result.map_err(|e| fail(&e))
}

fn with_suffix(p: &Path, suffix: &str) -> std::path::PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    s.into()
}

/// Creates `dir` (and parents) and makes it private to this user (0700 on Unix).
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Writes a new file readable only by this user (0600 on Unix), replacing any old one.
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let _ = fs::remove_file(path);
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut f = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(data)?;
    f.sync_all()?;
    Ok(())
}

// ================================================================== DER
/// (tag, content start, content end) of the DER element at `i`.
fn tlv(buf: &[u8], i: usize) -> Result<(u8, usize, usize), &'static str> {
    if i + 2 > buf.len() {
        return Err("truncated DER");
    }
    let tag = buf[i];
    let mut len = buf[i + 1] as usize;
    let mut i = i + 2;
    if len & 0x80 != 0 {
        let n = len & 0x7F;
        if !(1..=4).contains(&n) || i + n > buf.len() {
            return Err("invalid DER length");
        }
        len = buf[i..i + n]
            .iter()
            .fold(0usize, |acc, b| (acc << 8) | *b as usize);
        i += n;
    }
    if i + len > buf.len() {
        return Err("truncated DER");
    }
    Ok((tag, i, i + len))
}

/// The fields of a certificate's TBSCertificate: (tag, start of the element, end).
fn tbs_fields(cert_der: &[u8]) -> Result<Vec<(u8, usize, usize)>, &'static str> {
    let (tag, s, _e) = tlv(cert_der, 0)?;
    if tag != 0x30 {
        return Err("not a certificate");
    }
    let (tag, s2, e2) = tlv(cert_der, s)?;
    if tag != 0x30 {
        return Err("not a certificate");
    }
    let mut fields = Vec::new();
    let mut i = s2;
    while i < e2 {
        let (t, _cs, ce) = tlv(cert_der, i)?;
        fields.push((t, i, ce));
        i = ce;
    }
    let first = usize::from(fields.first().is_some_and(|f| f.0 == 0xA0)); // optional [0] version
    if fields.len() < first + 6 {
        return Err("incomplete certificate");
    }
    Ok(fields[first..].to_vec())
}

/// SubjectPublicKeyInfo (full DER element) of an X.509 certificate.
pub fn spki_der(cert_der: &[u8]) -> Result<&[u8], &'static str> {
    let fields = tbs_fields(cert_der)?;
    let (t, start, end) = fields[5]; // serial, sigAlg, issuer, validity, subject, SPKI
    if t != 0x30 {
        return Err("certificate without a public key");
    }
    Ok(&cert_der[start..end])
}

/// (notBefore, notAfter) of a certificate, in seconds since the epoch.
pub fn validity(cert_der: &[u8]) -> Result<(i64, i64), &'static str> {
    let fields = tbs_fields(cert_der)?;
    let (t, start, end) = fields[3];
    if t != 0x30 {
        return Err("certificate without validity");
    }
    let body = &cert_der[start..end];
    let (_t, s, e) = tlv(body, 0)?;
    let inner = &body[s..e];
    let (t1, s1, e1) = tlv(inner, 0)?;
    let (t2, s2, e2) = tlv(inner, e1)?;
    Ok((der_time(t1, &inner[s1..e1])?, der_time(t2, &inner[s2..e2])?))
}

/// UTCTime (YYMMDDHHMMSSZ) or GeneralizedTime (YYYYMMDDHHMMSSZ) → Unix seconds.
fn der_time(tag: u8, b: &[u8]) -> Result<i64, &'static str> {
    let text = std::str::from_utf8(b).map_err(|_| "bad time")?;
    let digits = text.strip_suffix('Z').ok_or("bad time")?;
    if !digits.bytes().all(|c| c.is_ascii_digit()) {
        return Err("bad time");
    }
    let num = |a: usize, b: usize| digits[a..b].parse::<i64>().map_err(|_| "bad time");
    let (year, rest) = match (tag, digits.len()) {
        (0x17, 12) => {
            let yy = num(0, 2)?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, 2)
        }
        (0x18, 14) => (num(0, 4)?, 4),
        _ => return Err("bad time"),
    };
    let month = num(rest, rest + 2)?;
    let day = num(rest + 2, rest + 4)?;
    let hour = num(rest + 4, rest + 6)?;
    let minute = num(rest + 6, rest + 8)?;
    let second = num(rest + 8, rest + 10)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return Err("bad time");
    }
    Ok(days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Hex SHA-256 of the certificate's SubjectPublicKeyInfo.
pub fn pin_of_der(cert_der: &[u8]) -> Result<String, &'static str> {
    Ok(hex(&sha256(spki_der(cert_der)?)))
}

pub fn pem_to_der(pem: &str) -> Result<Vec<u8>, &'static str> {
    let trimmed = pem.trim();
    if !trimmed.starts_with("-----BEGIN CERTIFICATE-----")
        || !trimmed.ends_with("-----END CERTIFICATE-----")
    {
        return Err("invalid PEM certificate");
    }
    CertificateDer::from_pem_slice(trimmed.as_bytes())
        .map(|c| c.as_ref().to_vec())
        .map_err(|_| "invalid PEM certificate")
}

pub fn pin_of_pem(pem: &str) -> Result<String, &'static str> {
    pin_of_der(&pem_to_der(pem)?)
}

/// Human-readable start of a pin: "A1B2 C3D4 E5F6 0718".
pub fn short_fingerprint(pin: &str) -> String {
    let p: Vec<char> = pin.to_uppercase().chars().take(16).collect();
    p.chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

// ================================================================== pairing
pub fn new_nonce() -> [u8; NONCE_BYTES] {
    random_bytes::<NONCE_BYTES>()
}

/// H("linoptions-flow-commit-v1\0" ‖ role ‖ "\0" ‖ own SPKI pin ‖ nonce): binds the nonce
/// to who sent it.
pub fn commitment(role: &str, pin: &str, nonce: &[u8]) -> String {
    let mut data = crate::wire::COMMIT_CONTEXT.to_vec();
    data.extend_from_slice(role.as_bytes());
    data.push(0);
    data.extend_from_slice(&unhex(pin).unwrap_or_default());
    data.extend_from_slice(nonce);
    hex(&sha256(&data))
}

/// 6-digit short authentication string both users compare.
pub fn sas(
    pin_initiator: &str,
    pin_responder: &str,
    nonce_initiator: &[u8],
    nonce_responder: &[u8],
) -> String {
    let mut data = crate::wire::SAS_CONTEXT.to_vec();
    data.extend_from_slice(&unhex(pin_initiator).unwrap_or_default());
    data.extend_from_slice(&unhex(pin_responder).unwrap_or_default());
    data.extend_from_slice(nonce_initiator);
    data.extend_from_slice(nonce_responder);
    let h = sha256(&data);
    let n = u64::from_be_bytes(h[..8].try_into().expect("8 bytes"));
    format!("{:06}", n % 1_000_000)
}

/// Text for an OS error (an `IdentityError` from an io::Error).
pub fn io_error(e: &std::io::Error) -> IdentityError {
    IdentityError(plain(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sas_and_commitment() {
        let (a, b) = ("11".repeat(32), "22".repeat(32));
        let (n1, n2) = ([1u8; 32], [2u8; 32]);
        let code = sas(&a, &b, &n1, &n2);
        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|c| c.is_ascii_digit()));
        assert_eq!(code, sas(&a, &b, &n1, &n2));
        assert_ne!(code, sas(&b, &a, &n1, &n2)); // roles matter
        assert_ne!(code, sas(&"33".repeat(32), &b, &n1, &n2)); // every pin matters
        assert_ne!(
            commitment("initiator", &a, &n1),
            commitment("initiator", &b, &n1)
        );
        assert_ne!(
            commitment("initiator", &a, &n1),
            commitment("responder", &a, &n1)
        );
        // Known values computed with the Python implementation.
        assert_eq!(code, "984931");
        assert_eq!(
            commitment("initiator", &a, &n1),
            "aa0f9e4b1d61a8c69a793bf385dcdf07a090e060cc518f513811689bd4d94bc3"
        );
        assert_eq!(
            commitment("responder", &b, &n2),
            "852ed1977586f064159daca0d5b0c2bdf1c09cebb05ebb260d487c64b1dd4d04"
        );
        assert_eq!(
            short_fingerprint("a1b2c3d4e5f60718ffff"),
            "A1B2 C3D4 E5F6 0718"
        );
    }

    #[test]
    fn dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(der_time(0x17, b"200101000000Z"), Ok(1_577_836_800));
        assert_eq!(der_time(0x18, b"20500101000000Z"), Ok(2_524_608_000));
        assert!(der_time(0x17, b"2001010000Z").is_err());
    }

    #[test]
    fn identity_is_p256_and_private() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("flow");
        let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
        generate_identity(&cert, &key).unwrap();
        let pem = fs::read_to_string(&cert).unwrap();
        let der = pem_to_der(&pem).unwrap();
        let spki = spki_der(&der).unwrap();
        // id-ecPublicKey + prime256v1
        let ec = [0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
        let p256 = [0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
        assert!(spki.windows(ec.len()).any(|w| w == ec));
        assert!(spki.windows(p256.len()).any(|w| w == p256));
        let (nb, na) = validity(&der).unwrap();
        assert_eq!(nb, 1_577_836_800);
        assert!(na > 1_577_836_800 + 24 * 365 * 86400);
        assert_eq!(pin_of_pem(&pem).unwrap().len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&cert), 0o600);
            assert_eq!(mode(&key), 0o600);
            assert_eq!(mode(&dir), 0o700);
        }
        assert!(fs::read_to_string(&key).unwrap().contains("PRIVATE KEY"));
    }
}
