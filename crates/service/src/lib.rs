//! The Glisa device service: finds Logitech devices, applies their settings, runs
//! button actions and answers the JSON API, in-process for the Tauri shell and over
//! local IPC for `glisa-ctl`.
//!
//! ```ignore
//! let service = Service::new(ServiceOptions { events, ..Default::default() })?;
//! service.start()?;                                   // scan/hot-plug + IPC
//! let devices = service.handle("devices", &Value::Null, Some("es"))?;
//! service.set_flow(flow);                              // routes flow_* to Flow
//! service.stop();
//! ```
//!
//! Modules: [`config`] (settings file), [`actions`] (action model, presets per OS),
//! [`executor`] (actions run), [`device`] (one managed device), [`service`] (the
//! manager), [`api`] (JSON API), [`ipc`] / [`client`] (local IPC), [`setup`] (Linux
//! udev rule), [`permissions`], [`models`] (family / photo).
//!
//! Note for Flow: `forget` deletes the device's settings but cannot yet delete Flow's
//! channel choices for it (the Python daemon called `flow.store.forget_device(key)`);
//! `FlowApi` has no method for it.

pub mod actions;
pub mod api;
pub mod client;
pub mod config;
pub mod device;
pub mod executor;
pub mod fsutil;
pub mod hidbackend;
pub mod ipc;
pub mod models;
pub mod permissions;
pub mod service;
pub mod setup;

pub use client::{Client, ClientError};
pub use config::Config;
pub use device::{DevError, ManagedDevice, UNNAMED_DEVICE};
pub use hidbackend::{HidBackend, NoHid, SystemHid};
pub use service::{Service, ServiceOptions};
