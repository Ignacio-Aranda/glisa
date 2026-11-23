//! Read-only hardware smoke test: lists the Logitech HID++ nodes, pings each device and
//! prints its name, type, ids and feature table. Changes nothing on the device; uses a
//! software id of its own, so it can run next to the service.
//!
//! `cargo run -p glisa-hid --example smoke`

use std::sync::Arc;
use std::time::Duration;

use glisa_hidpp::constants::feature_label;
use glisa_hidpp::{Channel, Device, features as fe};

/// Neither the Python service's (0x0A) nor the Rust service's (0x0B).
const SMOKE_SW_ID: u8 = 0x0E;

fn main() {
    let nodes = glisa_hid::scan();
    if nodes.is_empty() {
        println!("no Logitech HID++ nodes found");
        return;
    }
    for n in nodes {
        println!(
            "== {} {:?} bus={:#x} pid={:04X} short={} long={} receiver={}",
            n.path,
            n.name,
            n.bus,
            n.product,
            n.has_short,
            n.has_long,
            n.is_receiver()
        );
        let t = match glisa_hid::open(&n) {
            Ok(t) => t,
            Err(e) => {
                println!("   cannot open: {e}");
                continue;
            }
        };
        let ch = Channel::with_sw_id(Arc::from(t), SMOKE_SW_ID);
        let indices: Vec<u8> = if n.is_receiver() {
            (1..=6).collect()
        } else {
            vec![0xFF]
        };
        for idx in indices {
            let d = Device::new(ch.clone(), idx, &n.path, n.product);
            let Some(proto) = d.ping(Duration::from_secs(1)) else {
                continue;
            };
            println!("   [{idx:#04x}] HID++ {}.{}", proto.0, proto.1);
            if proto.0 < 2 {
                continue;
            }
            match fe::device_name(&d) {
                Ok(name) => println!("   name: {name} | type: {}", fe::device_type(&d)),
                Err(e) => println!("   name: -- ({e})"),
            }
            match fe::device_ids(&d) {
                Ok(ids) => println!("   ids: {ids:04X?}"),
                Err(e) => println!("   ids: -- ({e})"),
            }
            match d.enumerate_features() {
                Ok(feats) => println!(
                    "   features: {}",
                    feats
                        .iter()
                        .map(|(fid, _)| feature_label(*fid))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Err(e) => println!("   features: -- ({e})"),
            }
            match fe::battery(&d) {
                Ok(b) => println!("   battery: {b:?}"),
                Err(e) => println!("   battery: -- ({e})"),
            }
            d.detach();
        }
        ch.close();
    }
}
