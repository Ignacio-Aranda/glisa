//! Errors of HID++ requests.

use crate::constants::{error_10_name, error_20_name};

/// The transport went away (device unplugged, node closed, simulator disconnected).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("transport closed: {0}")]
pub struct TransportClosed(pub String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The device answered with an error. `text` is the protocol's English name
    /// ("out of range"); it is shown to people inside a translated sentence.
    #[error("HID++ error {code}: {text}")]
    Hidpp { code: u8, text: &'static str },
    /// No answer within the timeout (asleep, out of range, switched to another host).
    #[error("{0}")]
    NoResponse(String),
    /// The device does not have the feature.
    #[error("feature {0:#06x}")]
    FeatureNotSupported(u16),
    #[error(transparent)]
    Closed(#[from] TransportClosed),
    /// The answer was shorter than the protocol says (a broken or hostile device).
    #[error("malformed answer: {0}")]
    Malformed(&'static str),
}

impl Error {
    pub fn hidpp20(code: u8) -> Self {
        Error::Hidpp {
            code,
            text: error_20_name(code),
        }
    }

    pub fn hidpp10(code: u8) -> Self {
        Error::Hidpp {
            code,
            text: error_10_name(code),
        }
    }

    /// "The device didn't answer" kind of failure (as opposed to a refusal).
    pub fn is_unreachable(&self) -> bool {
        matches!(self, Error::NoResponse(_) | Error::Closed(_))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
