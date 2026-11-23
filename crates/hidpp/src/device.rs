//! A HID++ 2.0 device (Bluetooth/wired direct, or paired to a receiver) and its feature
//! table.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::channel::{Channel, DEFAULT_TIMEOUT, ListenerId};
use crate::constants::{DEVICE_INDEX_DIRECT, f};
use crate::error::{Error, Result};
use crate::transport::lock;

/// `(feature id, function, payload)` of a notification from this device.
pub type DeviceListener = Arc<dyn Fn(u16, u8, &[u8]) + Send + Sync>;

#[derive(Default)]
struct FeatureTable {
    /// feature id -> index (None: the device said it does not have it).
    by_id: HashMap<u16, Option<u8>>,
    by_index: HashMap<u8, u16>,
}

pub struct Device {
    channel: Arc<Channel>,
    pub devidx: u8,
    pub path: String,
    pub product_id: u16,
    table: Mutex<FeatureTable>,
    protocol: Mutex<Option<(u8, u8)>>,
    listeners: Mutex<Vec<(u64, DeviceListener)>>,
    next_id: AtomicU64,
    channel_listener: Mutex<Option<ListenerId>>,
}

impl Device {
    pub fn new(channel: Arc<Channel>, devidx: u8, path: &str, product_id: u16) -> Arc<Self> {
        let mut table = FeatureTable::default();
        table.by_id.insert(f::ROOT, Some(0));
        table.by_index.insert(0, f::ROOT);
        let dev = Arc::new(Device {
            channel: channel.clone(),
            devidx,
            path: path.to_string(),
            product_id,
            table: Mutex::new(table),
            protocol: Mutex::new(None),
            listeners: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(0),
            channel_listener: Mutex::new(None),
        });
        let weak = Arc::downgrade(&dev);
        let id = channel.add_listener(Arc::new(move |devidx, feat_index, function, data| {
            if let Some(d) = weak.upgrade() {
                d.on_notification(devidx, feat_index, function, data);
            }
        }));
        *lock(&dev.channel_listener) = Some(id);
        dev
    }

    /// A device answering as index 0xFF (Bluetooth, USB cable).
    pub fn direct(channel: Arc<Channel>, path: &str, product_id: u16) -> Arc<Self> {
        Self::new(channel, DEVICE_INDEX_DIRECT, path, product_id)
    }

    pub fn channel(&self) -> &Arc<Channel> {
        &self.channel
    }

    /// Protocol version found by the last successful [`Device::ping`].
    pub fn protocol(&self) -> Option<(u8, u8)> {
        *lock(&self.protocol)
    }

    // -------------------------------------------------------------- notifications

    pub fn add_listener(&self, f: DeviceListener) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        lock(&self.listeners).push((id, f));
        id
    }

    pub fn remove_listener(&self, id: u64) {
        lock(&self.listeners).retain(|(i, _)| *i != id);
    }

    /// Stops listening on the channel (the channel itself stays open: other devices may
    /// share it).
    pub fn detach(&self) {
        lock(&self.listeners).clear();
        if let Some(id) = lock(&self.channel_listener).take() {
            self.channel.remove_listener(id);
        }
    }

    fn on_notification(&self, devidx: u8, feat_index: u8, function: u8, data: &[u8]) {
        if devidx != self.devidx {
            return;
        }
        let Some(fid) = lock(&self.table).by_index.get(&feat_index).copied() else {
            return;
        };
        let listeners: Vec<DeviceListener> = lock(&self.listeners)
            .iter()
            .map(|(_, f)| f.clone())
            .collect();
        for cb in listeners {
            cb(fid, function, data);
        }
    }

    // ----------------------------------------------------------------------- core

    /// Protocol version, `(1, 0)` for a HID++ 1.0 device, `None` when nothing answers.
    pub fn ping(&self, timeout: Duration) -> Option<(u8, u8)> {
        match self
            .channel
            .request(self.devidx, 0, 1, &[0, 0, 0x5A], timeout)
        {
            Ok(r) => {
                let p = (r[0], r[1]);
                *lock(&self.protocol) = Some(p);
                Some(p)
            }
            // A HID++ 1.0 device answers "invalid sub id".
            Err(Error::Hidpp { code: 1, .. }) => {
                *lock(&self.protocol) = Some((1, 0));
                Some((1, 0))
            }
            Err(_) => None,
        }
    }

    pub fn feature_index(&self, feature_id: u16) -> Result<Option<u8>> {
        if let Some(idx) = lock(&self.table).by_id.get(&feature_id) {
            return Ok(*idx);
        }
        let r = self.channel.request(
            self.devidx,
            0,
            0,
            &feature_id.to_be_bytes(),
            DEFAULT_TIMEOUT,
        )?;
        let idx = (r[0] != 0).then_some(r[0]);
        let mut t = lock(&self.table);
        t.by_id.insert(feature_id, idx);
        if let Some(i) = idx {
            t.by_index.insert(i, feature_id);
        }
        Ok(idx)
    }

    pub fn has(&self, feature_id: u16) -> bool {
        matches!(self.feature_index(feature_id), Ok(Some(_)))
    }

    pub fn call(&self, feature_id: u16, function: u8, params: &[u8]) -> Result<Vec<u8>> {
        self.call_timeout(feature_id, function, params, DEFAULT_TIMEOUT)
    }

    pub fn call_timeout(
        &self,
        feature_id: u16,
        function: u8,
        params: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let idx = self
            .feature_index(feature_id)?
            .ok_or(Error::FeatureNotSupported(feature_id))?;
        self.channel
            .request(self.devidx, idx, function, params, timeout)
    }

    /// `{feature id: index}` for every feature the device reports.
    pub fn enumerate_features(&self) -> Result<Vec<(u16, u8)>> {
        let count = self.call(f::FEATURE_SET, 0, &[])?[0];
        let mut out = Vec::with_capacity(count as usize);
        for i in 1..=count {
            let r = self.call(f::FEATURE_SET, 1, &[i])?;
            let fid = u16::from_be_bytes([r[0], r[1]]);
            out.push((fid, i));
            let mut t = lock(&self.table);
            t.by_id.insert(fid, Some(i));
            t.by_index.insert(i, fid);
        }
        Ok(out)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if let Some(id) = lock(&self.channel_listener).take() {
            self.channel.remove_listener(id);
        }
    }
}
