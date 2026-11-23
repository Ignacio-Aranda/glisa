//! Peer addresses typed by the user: `host[:port]`, where host is an IPv4 address, an IPv6
//! address (in brackets when a port follows) or a host name. Addresses are only ever hints:
//! the pinned TLS session decides who answers.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use glisa_i18n::tr;

use crate::PORT;

/// Longer than any host name (253) + brackets + port.
pub const MAX_INPUT: usize = 300;

/// Empty or `auto`: no manual address (find it automatically).
pub fn is_auto(text: Option<&str>) -> bool {
    match text {
        None => true,
        Some(t) => {
            let t = t.trim().to_lowercase();
            t.is_empty() || t == "auto"
        }
    }
}

fn parse_port(text: &str) -> Result<u16, String> {
    let bad = || tr!("The port must be a number from 1 to 65535");
    if text.is_empty() || text.len() > 5 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    match text.parse::<u32>() {
        Ok(p) if (1..=65535).contains(&p) => Ok(p as u16),
        _ => Err(bad()),
    }
}

fn label_ok(l: &str) -> bool {
    let b = l.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
}

fn hostname_ok(host: &str) -> bool {
    let name = host.strip_suffix('.').unwrap_or(host);
    if name.is_empty() || name.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = name.split('.').collect();
    if !labels.iter().all(|l| label_ok(l)) {
        return false;
    }
    // 300.1.1.1 is a bad IPv4 address, not a name
    !labels
        .last()
        .is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
}

/// A host as stored (no brackets): IPv4, IPv6 or a host name.
pub fn valid_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    !host.contains(':') && hostname_ok(host)
}

fn short(host: &str) -> String {
    host.chars().take(64).collect()
}

/// `host[:port]` → (host, port); None for empty / `auto`. Errors are translated (this
/// thread's language). IPv6 comes back without brackets.
pub fn parse_address(text: &str) -> Result<Option<(String, u16)>, String> {
    let text = text.trim();
    if text.chars().count() > MAX_INPUT {
        return Err(tr!("That address is too long"));
    }
    if is_auto(Some(text)) {
        return Ok(None);
    }
    let mut port = PORT;
    if let Some(rest) = text.strip_prefix('[') {
        let Some((host, after)) = rest.split_once(']') else {
            return Err(tr!(
                "An IPv6 address needs its closing bracket: [address]:port"
            ));
        };
        if !after.is_empty() {
            let Some(p) = after.strip_prefix(':') else {
                return Err(tr!("Invalid address"));
            };
            port = parse_port(p)?;
        }
        if host.parse::<Ipv6Addr>().is_err() {
            return Err(tr!(
                "“{host}” isn't a valid IPv6 address",
                host = short(host)
            ));
        }
        return Ok(Some((host.to_string(), port)));
    }
    if text.matches(':').count() > 1 {
        // a bare IPv6 address (no port possible)
        if text.parse::<Ipv6Addr>().is_err() {
            return Err(tr!("Put an IPv6 address in brackets, like [fe80::1]:27461"));
        }
        return Ok(Some((text.to_string(), port)));
    }
    let (host, rest) = match text.split_once(':') {
        Some((h, r)) => (h, Some(r)),
        None => (text, None),
    };
    if let Some(r) = rest {
        port = parse_port(r)?;
    }
    if host.is_empty() {
        return Err(tr!("Type the computer's IP address or name"));
    }
    if host
        .split('.')
        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    {
        return match host.parse::<Ipv4Addr>() {
            Ok(ip) => Ok(Some((ip.to_string(), port))),
            Err(_) => Err(tr!("“{host}” isn't a valid IP address", host = short(host))),
        };
    }
    if !hostname_ok(host) {
        return Err(tr!(
            "“{host}” isn't a valid computer name or IP address",
            host = short(host)
        ));
    }
    Ok(Some((host.to_lowercase(), port)))
}

/// (host, port) → "host:port" ("[v6]:port"); "" for nothing.
pub fn format_address(addr: Option<&(String, u16)>) -> String {
    match addr {
        None => String::new(),
        Some((host, port)) if host.contains(':') => format!("[{host}]:{port}"),
        Some((host, port)) => format!("{host}:{port}"),
    }
}

/// An IPv4 literal (discovery is IPv4, and no name lookups on its threads).
pub fn is_ipv4(host: &str) -> bool {
    host.parse::<Ipv4Addr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn good() {
        let cases = [
            ("192.168.2.20", ("192.168.2.20", PORT)),
            (" 192.168.2.20:27500 ", ("192.168.2.20", 27500)),
            ("[fe80::1]", ("fe80::1", PORT)),
            ("[fe80::1]:9", ("fe80::1", 9)),
            ("fe80::1", ("fe80::1", PORT)),
            ("Portatil-Ana.local", ("portatil-ana.local", PORT)),
            ("pc:65535", ("pc", 65535)),
            ("pc.:1", ("pc.", 1)),
        ];
        for (text, (h, p)) in cases {
            assert_eq!(
                parse_address(text).unwrap(),
                Some((h.to_string(), p)),
                "{text}"
            );
        }
        for text in ["", "   ", "auto", "AUTO"] {
            assert_eq!(parse_address(text).unwrap(), None);
        }
        assert_eq!(format_address(Some(&("fe80::1".into(), 9))), "[fe80::1]:9");
        assert_eq!(
            format_address(Some(&("10.0.0.2".into(), 27461))),
            "10.0.0.2:27461"
        );
        assert_eq!(format_address(None), "");
    }

    #[test]
    fn bad() {
        let long_label = format!("{}.local", "x".repeat(64));
        let many = "a.".repeat(130);
        let huge = "x".repeat(301);
        for text in [
            "300.1.1.1",
            "1.2.3",
            "10.0.0.1:0",
            "10.0.0.1:65536",
            "10.0.0.1:",
            "10.0.0.1:x",
            "pc:+1",
            "[fe80::1",
            "[10.0.0.1]:5",
            "[fe80::1]5",
            "fe80::1::2",
            ":27461",
            "bad host",
            "a_b",
            "-pc",
            "pc..local",
            &long_label,
            &many,
            &huge,
            "pc/../etc",
            "pc\0",
            "<b>pc</b>",
        ] {
            assert!(parse_address(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn errors_are_translated() {
        glisa_i18n::with_language(Some("en"), || {
            assert_eq!(
                parse_address("pc:0").unwrap_err(),
                "The port must be a number from 1 to 65535"
            );
        });
        glisa_i18n::with_language(Some("es"), || {
            assert_eq!(
                parse_address("300.1.1.1").unwrap_err(),
                "«300.1.1.1» no es una dirección IP válida"
            );
        });
    }
}
