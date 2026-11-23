//! `glisa-ctl probe`: talks to the hardware directly, without the service, and
//! dumps what it finds. Read-only, with a software id of its own (0x0C), so it works
//! next to a running service.

use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

use glisa_hidpp::constants::feature_label;
use glisa_hidpp::{Channel, Device, features as fe};
use glisa_i18n::{n_, tr};

const PROBE_SW_ID: u8 = 0x0C;

fn show<T: std::fmt::Debug>(
    out: &mut dyn Write,
    label: &str,
    r: Result<T, glisa_hidpp::Error>,
) -> io::Result<()> {
    match r {
        Ok(v) => writeln!(out, "   {label}: {v:?}"),
        Err(e) => writeln!(out, "   {label}: -- ({e})"),
    }
}

pub fn run(out: &mut dyn Write) -> io::Result<u8> {
    let nodes = glisa_hid::scan();
    if nodes.is_empty() {
        writeln!(
            out,
            "{}",
            tr!("No Logitech hidraw nodes found. Is the device awake and connected?")
        )?;
        return Ok(1);
    }
    for n in nodes {
        writeln!(
            out,
            "== {}  {}  bus={:#x} pid={:04X} short={} long={} receiver={}",
            n.path,
            n.name,
            n.bus,
            n.product,
            n.has_short,
            n.has_long,
            n.is_receiver()
        )?;
        let t = match glisa_hid::open(&n) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                writeln!(
                    out,
                    "   {}",
                    tr!("no permission: run ./install.sh (udev rules)")
                )?;
                continue;
            }
            Err(e) => {
                writeln!(out, "   {e}")?;
                continue;
            }
        };
        let ch = Channel::with_sw_id(Arc::from(t), PROBE_SW_ID);
        let indices: Vec<u8> = if n.is_receiver() {
            (1..=6).collect()
        } else {
            vec![0xFF]
        };
        for idx in indices {
            let d = Device::new(ch.clone(), idx, &n.path, n.product);
            let Some(proto) = d.ping(Duration::from_secs(1)) else {
                d.detach();
                continue;
            };
            writeln!(out, "   [{idx:#04x}] HID++ {}.{}", proto.0, proto.1)?;
            if proto.0 < 2 {
                d.detach();
                continue;
            }
            let feats = d.enumerate_features().unwrap_or_default();
            writeln!(
                out,
                "   {}",
                tr!(
                    "name: {name} | type: {kind}",
                    name = fe::device_name(&d).unwrap_or_default(),
                    kind = fe::device_type(&d)
                )
            )?;
            let list: Vec<String> = feats.iter().map(|(fid, _)| feature_label(*fid)).collect();
            writeln!(
                out,
                "   {}",
                tr!("features: {list}", list = list.join(", "))
            )?;
            show(out, "ids", fe::device_ids(&d))?;
            show(out, &tr!(n_!("battery")), fe::battery(&d))?;
            show(out, "DPI", fe::dpi_info(&d, 0))?;
            show(out, &tr!(n_!("wheel")), fe::wheel_info(&d))?;
            show(out, "SmartShift", fe::smartshift_info(&d))?;
            show(out, &tr!(n_!("channels")), fe::host_info(&d))?;
            show(out, "firmware", fe::firmware(&d))?;
            match fe::controls(&d) {
                Ok(cs) => {
                    for c in cs {
                        writeln!(
                            out,
                            "   control {:#06x} {:<26} reprog={} divert={} rawxy={} flags={:#04x}",
                            c.cid,
                            c.name,
                            c.reprogrammable(),
                            c.divertable(),
                            c.supports_raw_xy(),
                            c.flags
                        )?;
                    }
                }
                Err(e) => writeln!(out, "   {}", tr!("controls: -- {error}", error = e))?,
            }
            d.detach();
        }
        ch.close();
    }
    Ok(0)
}
