//! HID++ over the simulator.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glisa_hidpp::constants::{REPORT_LONG, SW_ID};
use glisa_hidpp::sim::SimulatedMouse;
use glisa_hidpp::{Channel, Device, Error, Transport, f, features as fe};

fn wait_for(mut pred: impl FnMut() -> bool, timeout: Duration) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

struct Setup {
    sim: Arc<SimulatedMouse>,
    ch: Arc<Channel>,
    dev: Arc<Device>,
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.ch.close();
    }
}

fn setup(profile: &str) -> Setup {
    let sim = SimulatedMouse::new(profile, 0xFF);
    let ch = Channel::new(sim.clone());
    let dev = Device::new(ch.clone(), 0xFF, "sim", sim.product_id);
    Setup { sim, ch, dev }
}

#[test]
fn ping_and_name() {
    let s = setup("lift");
    assert_eq!(s.dev.ping(Duration::from_secs(1)), Some((4, 5)));
    assert_eq!(fe::device_name(&s.dev).unwrap(), "LIFT");
    assert_eq!(fe::device_type(&s.dev), "mouse");
    assert_eq!(fe::firmware(&s.dev).unwrap(), vec!["FW RBM16.01.B0017"]);
}

#[test]
fn requests_use_our_software_id_and_long_reports() {
    let s = setup("lift");
    s.dev.ping(Duration::from_secs(1));
    let log = s.sim.log.lock().unwrap();
    let req = log.last().unwrap();
    assert_eq!(req.len(), 20);
    assert_eq!(req[0], REPORT_LONG);
    assert_eq!(req[3] & 0x0F, SW_ID);
    assert_ne!(SW_ID, 0x0A, "the Python service uses 0x0A");
}

#[test]
fn dpi_range_expansion() {
    let s = setup("lift");
    let info = fe::dpi_info(&s.dev, 0).unwrap();
    assert_eq!(
        (info.minimum, info.maximum, info.step),
        (400, 4000, Some(100))
    );
    assert!(info.choices.contains(&2500));
    assert_eq!(fe::set_dpi(&s.dev, 2500, 0).unwrap(), 2500);
    assert_eq!(s.sim.state().dpi, 2500);
}

#[test]
fn error_response() {
    let s = setup("lift");
    match fe::set_dpi(&s.dev, 9000, 0) {
        Err(Error::Hidpp { code, text }) => {
            assert_eq!(code, 3);
            assert_eq!(text, "out of range");
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn missing_feature() {
    let s = setup("keys");
    assert!(!s.dev.has(f::ADJUSTABLE_DPI));
    assert_eq!(
        fe::dpi_info(&s.dev, 0),
        Err(Error::FeatureNotSupported(f::ADJUSTABLE_DPI))
    );
    assert_eq!(fe::device_type(&s.dev), "keyboard");
}

#[test]
fn controls_and_divert() {
    let s = setup("lift");
    let cs = fe::controls(&s.dev).unwrap();
    let get = |cs: &[fe::Control], cid| cs.iter().find(|c| c.cid == cid).cloned().unwrap();
    assert!(get(&cs, 0x53).divertable());
    assert!(!get(&cs, 0x50).divertable());
    assert_eq!(get(&cs, 0x53).group_cids, vec![0x50, 0x51, 0x52, 0x56]);
    fe::set_control_reporting(&s.dev, 0x53, true, true, 0).unwrap();
    let cs = fe::controls(&s.dev).unwrap();
    assert!(get(&cs, 0x53).diverted && get(&cs, 0x53).raw_xy);
}

#[test]
fn notifications_reach_listener() {
    let s = setup("lift");
    let got: Arc<Mutex<Vec<(u16, u8)>>> = Arc::default();
    s.dev.enumerate_features().unwrap();
    let g = got.clone();
    s.dev.add_listener(Arc::new(move |fid, func, _d| {
        g.lock().unwrap().push((fid, func))
    }));
    fe::set_control_reporting(&s.dev, 0x53, true, false, 0).unwrap();
    s.sim.press(0x53);
    assert!(wait_for(
        || !got.lock().unwrap().is_empty(),
        Duration::from_secs(2)
    ));
    assert_eq!(got.lock().unwrap()[0], (0x1B04, 0));
}

#[test]
fn hosts_and_smartshift() {
    let s = setup("mx");
    assert_eq!(fe::host_info(&s.dev).unwrap(), (3, 0));
    let entries = fe::host_entries(&s.dev, 3);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].paired, Some(true));
    assert!(!entries[0].name.is_empty());
    assert_eq!(entries[1].name, "MacBook de trabajo");
    assert_eq!(entries[2].paired, Some(false));
    let ss = fe::smartshift_info(&s.dev).unwrap().unwrap();
    assert!(ss.enhanced && ss.ratchet && ss.threshold == 30);
    fe::set_smartshift(&s.dev, false, 300, ss.torque).unwrap();
    assert_eq!(s.sim.state().ss, [1, 255, 50]);
    let w = fe::wheel_info(&s.dev).unwrap();
    assert!(w.has_invert && !w.inverted);
    fe::set_wheel_mode(&s.dev, true, true, false).unwrap();
    assert_eq!(s.sim.state().wheel_mode, 0x06);
    let b = fe::battery(&s.dev).unwrap().unwrap();
    assert_eq!((b.percent, b.level, b.charging), (Some(42), "good", false));
}

#[test]
fn set_host_switches_without_an_answer() {
    let s = setup("lift");
    let t0 = Instant::now();
    fe::set_host(&s.dev, 1);
    assert!(t0.elapsed() < Duration::from_secs(1));
    assert_eq!(s.sim.state().host, 1);
}

#[test]
fn asleep_device_does_not_answer() {
    let s = setup("lift");
    s.sim.set_asleep(true);
    assert_eq!(s.dev.ping(Duration::from_millis(200)), None);
    s.sim.set_asleep(false);
    assert!(s.dev.ping(Duration::from_millis(500)).is_some());
}

#[test]
fn disconnect_closes_the_channel_and_calls_listeners() {
    let s = setup("lift");
    let closed = Arc::new(Mutex::new(false));
    let c = closed.clone();
    s.ch.add_closed_listener(Arc::new(move || *c.lock().unwrap() = true));
    s.sim.disconnect();
    assert!(wait_for(|| *closed.lock().unwrap(), Duration::from_secs(2)));
    assert!(s.ch.is_closed());
    assert!(s.ch.join_reader(Duration::from_secs(2)));
    assert!(s.dev.ping(Duration::from_millis(100)).is_none());
}

#[test]
fn close_stops_the_reader_without_calling_listeners() {
    let s = setup("lift");
    let closed = Arc::new(Mutex::new(false));
    let c = closed.clone();
    s.ch.add_closed_listener(Arc::new(move || *c.lock().unwrap() = true));
    s.ch.close();
    assert!(s.ch.join_reader(Duration::from_secs(2)));
    assert!(!*closed.lock().unwrap());
}

#[test]
fn answers_for_another_software_id_are_ignored() {
    // Another program (the Python service, software id 0x0A) talks to the same node:
    // its answers must neither satisfy our request nor look like notifications.
    let s = setup("lift");
    let seen: Arc<Mutex<u32>> = Arc::default();
    let n = seen.clone();
    s.ch.add_listener(Arc::new(move |_, _, _, _| *n.lock().unwrap() += 1));
    let mut other = vec![0x11, 0xFF, 0x00, 0x1A];
    other.resize(20, 0);
    s.sim.write(&other).unwrap(); // the Python service pings
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(*seen.lock().unwrap(), 0);
    assert_eq!(s.dev.ping(Duration::from_secs(1)), Some((4, 5)));
}

#[test]
fn device_ids_from_firmware_feature() {
    let s = setup("lift");
    // The simulator answers only the entity count: no transport ids.
    assert_eq!(fe::device_ids(&s.dev).unwrap().all(), Vec::<u16>::new());
}

#[test]
fn diverted_and_raw_xy_parsing() {
    assert_eq!(
        fe::parse_diverted_buttons(&[0, 0x53, 0, 0xC3, 0, 0, 0, 0]),
        vec![0x53, 0xC3]
    );
    assert_eq!(fe::parse_raw_xy(&[0xFF, 0xCE, 0x00, 0x03]), (-50, 3));
    assert_eq!(fe::parse_raw_xy(&[1]), (0, 0));
}
