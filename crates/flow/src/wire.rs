//! WIRE CONSTANTS: shared with the Python peer; never rename.
//!
//! Everything another computer sees on the network. Computers that run the Python version
//! (or any earlier release) only understand these exact values, so a product rename must not
//! touch this block. The message types and field names are part of the wire too; they are
//! the schema in `proto.rs`.

/// Flow protocol version (`v` of every message and announcement).
pub const PROTOCOL_VERSION: u64 = 1;
/// UDP: discovery · TCP: authenticated peer channel and transfer connections.
pub const PORT: u16 = 27461;
/// TCP `port + 1`: pairing introduction, only while pairing is open.
pub const PAIR_PORT_OFFSET: u16 = 1;
/// `app` of every discovery packet.
pub const DISCOVERY_APP: &str = "linoptions-flow";
/// Common name (subject and issuer) of the identity certificate.
pub const CERT_COMMON_NAME: &str = "linoptions-flow";
/// Domain-separation prefixes of the pairing commitment and of the SAS.
pub const COMMIT_CONTEXT: &[u8] = b"linoptions-flow-commit-v1\0";
pub const SAS_CONTEXT: &[u8] = b"linoptions-flow-sas-v1\0";
/// Pairing roles inside the commitment.
pub const ROLE_INITIATOR: &str = "initiator";
pub const ROLE_RESPONDER: &str = "responder";
/// Capability announced in `hello.caps`: this computer accepts transfer connections.
pub const CAP_TRANSFER: &str = "transfer";
/// Capability announced in `hello.caps`: this computer takes `drag` notices (files dragged
/// across the edge are coming with the mouse) and gets ready to let the drag go on there.
pub const CAP_DRAG: &str = "drag";

#[cfg(test)]
mod tests {
    /// These values are what every paired computer already speaks: a rename must not
    /// change them (this test is the reminder).
    #[test]
    fn wire_values_never_change() {
        assert_eq!(super::DISCOVERY_APP, "linoptions-flow");
        assert_eq!(super::CERT_COMMON_NAME, "linoptions-flow");
        assert_eq!(super::COMMIT_CONTEXT, b"linoptions-flow-commit-v1\0");
        assert_eq!(super::SAS_CONTEXT, b"linoptions-flow-sas-v1\0");
        assert_eq!(
            (
                super::PORT,
                super::PAIR_PORT_OFFSET,
                super::PROTOCOL_VERSION
            ),
            (27461, 1, 1)
        );
        assert_eq!(
            (
                super::ROLE_INITIATOR,
                super::ROLE_RESPONDER,
                super::CAP_TRANSFER,
                super::CAP_DRAG
            ),
            ("initiator", "responder", "transfer", "drag")
        );
    }
}
