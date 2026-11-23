//! Minimal, dependency-free HID++ 1.0/2.0 implementation for Logitech devices: report
//! framing, the request/answer channel, the device feature table, the features Glisa uses,
//! and a simulator.
//!
//! The crate is synchronous: one reader thread per [`Channel`], requests block the
//! caller until their answer arrives or times out.

pub mod channel;
pub mod constants;
pub mod device;
pub mod error;
pub mod features;
pub mod sim;
pub mod transport;

pub use channel::{Channel, ClosedFn, DEFAULT_TIMEOUT, ListenerId, NotifyFn};
pub use constants::f;
pub use device::{Device, DeviceListener};
pub use error::{Error, Result, TransportClosed};
pub use transport::{ReportQueue, Transport};
