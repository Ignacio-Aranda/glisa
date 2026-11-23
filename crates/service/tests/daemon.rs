//! The device service over simulated devices.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use glisa_hidpp::Channel;
use glisa_hidpp::sim::SimulatedMouse;
use glisa_service::actions::default_gestures_for;
use serde_json::json;

struct Setup {
    _dir: TmpDir,
    cfg: std::path::PathBuf,
    s: Arc<glisa_service::Service>,
    sim: Arc<SimulatedMouse>,
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.s.stop();
    }
}

fn setup() -> Setup {
    let dir = TmpDir::new("daemon");
    let cfg = dir.path("c.json");
    let s = service(cfg.clone(), true);
    let sim = s.add_simulated("lift");
    Setup {
        _dir: dir,
        cfg,
        s,
        sim,
    }
}

fn linux_gestures() -> serde_json::Value {
    default_gestures_for(glisa_common::Platform::Linux)
}

#[test]
fn keys_held_while_pressed() {
    let t = setup();
    let md = t.s.device("b031-lift").unwrap();
    md.set_button(0x53, Some(&json!({"type": "keys", "combo": "Ctrl+C"})))
        .unwrap();
    t.sim.press(0x53);
    assert!(wait_for(|| key_events(&t.s).len() == 2, 2.0));
    assert_eq!(key_events(&t.s), vec![(1, 29, 1), (1, 46, 1)]);
    t.sim.release(0x53);
    assert!(wait_for(|| key_events(&t.s).len() == 4, 2.0));
    assert_eq!(key_events(&t.s)[2..], [(1, 46, 0), (1, 29, 0)]);
}

#[test]
fn gesture() {
    let t = setup();
    let md = t.s.device("b031-lift").unwrap();
    md.set_button(0xFD, Some(&linux_gestures())).unwrap();
    assert_ne!(t.sim.state().reporting[&0xFD].0 & 0x10, 0, "rawXY enabled");
    t.sim.press(0xFD);
    std::thread::sleep(Duration::from_millis(50));
    t.sim.move_by(-50, 3);
    t.sim.move_by(-50, 3);
    std::thread::sleep(Duration::from_millis(50));
    t.sim.release(0xFD);
    // Ctrl+Meta+Left tapped
    assert!(wait_for(|| key_events(&t.s).len() == 6, 2.0));
    let codes: Vec<u16> = key_events(&t.s)[..3].iter().map(|e| e.1).collect();
    assert_eq!(codes, vec![29, 125, 105]);
}

#[test]
fn gesture_tap() {
    let t = setup();
    let md = t.s.device("b031-lift").unwrap();
    md.set_button(0xFD, Some(&linux_gestures())).unwrap();
    t.sim.press(0xFD);
    std::thread::sleep(Duration::from_millis(50));
    t.sim.release(0xFD);
    assert!(wait_for(|| key_events(&t.s).len() == 4, 2.0));
    let codes: Vec<u16> = key_events(&t.s)[..2].iter().map(|e| e.1).collect();
    assert_eq!(codes, vec![125, 17]); // Meta+W
}

#[test]
fn native_mouse_remap_not_diverted() {
    let t = setup();
    let md = t.s.device("b031-lift").unwrap();
    md.set_button(0x56, Some(&json!({"type": "mouse", "button": "middle"})))
        .unwrap();
    assert_eq!(t.sim.state().reporting[&0x56], (0, 0x52));
    md.set_button(0x56, None).unwrap();
    assert_eq!(t.sim.state().reporting[&0x56], (0, 0));
}

#[test]
fn dpi_cycle() {
    let t = setup();
    let md = t.s.device("b031-lift").unwrap();
    md.set("dpi_presets", &json!([800, 1600, 2400])).unwrap();
    md.set("dpi", &json!(800)).unwrap();
    md.set_button(0xFD, Some(&json!({"type": "dpi_cycle"})))
        .unwrap();
    t.sim.press(0xFD);
    assert!(wait_for(|| t.sim.state().dpi == 1600, 2.0));
    t.sim.release(0xFD);
}

#[test]
fn settings_persist_and_reapply() {
    let t = setup();
    let md = t.s.device("b031-lift").unwrap();
    md.set("dpi", &json!(3200)).unwrap();
    md.set("wheel_inverted", &json!(true)).unwrap();
    md.set_button(0x53, Some(&json!({"type": "keys", "combo": "Ctrl+Z"})))
        .unwrap();
    // A new service (as after a restart) gives the device its settings back.
    let s2 = service(t.cfg.clone(), false);
    let sim2 = s2.add_simulated("lift");
    assert_eq!(sim2.state().dpi, 3200);
    assert_eq!(sim2.state().wheel_mode & 0x04, 0x04);
    assert_eq!(sim2.state().reporting[&0x53].0 & 1, 1);
    s2.stop();
}

#[test]
fn api_snapshot() {
    let t = setup();
    let snap = call(&t.s, "device", json!({"id": "b031-lift"})).unwrap();
    assert_eq!(snap["name"], json!("LIFT"));
    assert_eq!(snap["model"], json!("LIFT"));
    assert_eq!(snap["product_id"], json!("B031"));
    assert_eq!(snap["hosts"]["count"], json!(3));
    let cids: Vec<u64> = snap["controls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["cid"].as_u64().unwrap())
        .collect();
    assert_eq!(cids, vec![0x52, 0x53, 0x56, 0xFD]);
    assert_eq!(snap["controls"][1]["name"], json!("Back button"));
    assert_eq!(snap["controls"][1]["action"], json!({"type": "default"}));
    assert_eq!(snap["firmware"], json!(["FW RBM16.01.B0017"]));
    assert_eq!(snap["dpi"]["min"], json!(400));
    assert_eq!(snap["dpi"]["step"], json!(100));
    assert_eq!(snap["dpi"]["presets"], json!([800, 1600, 2400]));
    assert_eq!(
        snap["wheel"],
        json!({"hires": false, "inverted": false, "has_invert": true})
    );
    assert_eq!(snap["smartshift"], json!(null));
    assert_eq!(snap["battery"]["percent"], json!(78));
    assert_eq!(snap["settings"], json!({}));
    assert!(
        snap["features"]
            .as_array()
            .unwrap()
            .contains(&json!("ADJUSTABLE_DPI"))
    );
    // Additions: illustration family (from data/models.json: B031 is the LIFT).
    assert_eq!(snap["family"], json!("mouse-vertical"));
    assert!(snap.get("photo").is_some());
    let es =
        t.s.handle("device", &json!({"id": "b031-lift"}), Some("es"))
            .unwrap();
    assert_eq!(es["controls"][1]["name"], json!("Atrás"));
}

#[test]
fn host_names_and_connection() {
    let t = setup();
    let snap = call(&t.s, "device", json!({"id": "b031-lift"})).unwrap();
    let entries = snap["hosts"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert!(entries[0]["paired"].as_bool().unwrap());
    assert!(!entries[0]["name"].as_str().unwrap().is_empty());
    assert_eq!(entries[1]["name"], json!("MacBook de trabajo"));
    assert_eq!(entries[2]["paired"], json!(false));
    assert_eq!(snap["connection"], json!("bluetooth"));
}

#[test]
fn set_host_validates_and_switches() {
    let t = setup();
    let md = t.s.device("b031-lift").unwrap();
    assert!(md.set_host(7).is_err());
    assert_eq!(
        call(&t.s, "set_host", json!({"id": "b031-lift", "host": 7})),
        Err("Invalid channel".into())
    );
    md.set_host(1).unwrap();
    assert!(wait_for(|| t.sim.state().host == 1, 2.0));
}

#[test]
fn status_reports_setup_state() {
    let t = setup();
    let st = call(&t.s, "status", json!(null)).unwrap();
    assert!(st.get("rules_installed").is_some());
    assert_eq!(st["devices"], json!(1));
    assert_eq!(st["simulate"], json!(true));
    assert_eq!(st["denied"], json!([]));
    assert_eq!(st["platform"], json!("linux"));
    assert!(st["permissions"].is_array());
    assert_eq!(
        call(&t.s, "ping", json!({})).unwrap()["version"],
        json!(glisa_common::VERSION)
    );
}

#[test]
fn reset_restores_wheel_buttons_and_dpi() {
    let t = setup();
    let md = t.s.device("b031-lift").unwrap();
    md.set("dpi", &json!(3200)).unwrap();
    md.set("wheel_inverted", &json!(true)).unwrap();
    md.set("wheel_hires", &json!(true)).unwrap();
    md.set_button(0x53, Some(&json!({"type": "keys", "combo": "Ctrl+Z"})))
        .unwrap();
    assert_eq!(call(&t.s, "reset", json!({"id": md.id})), Ok(json!(true)));
    assert_eq!(t.sim.state().dpi, 1000);
    assert_eq!(t.sim.state().wheel_mode & 0x06, 0);
    assert_eq!(t.sim.state().reporting[&0x53], (0, 0));
    assert_eq!(t.s.config.device_copy("B031:LIFT")["buttons"], json!({}));
}

#[test]
fn reset_restores_smartshift() {
    let t = setup();
    let sim = t.s.add_simulated("mx");
    let md = t.s.device("b034-wireless_mouse_mx-sim").unwrap();
    let before = {
        let st = sim.state();
        [st.ss[0], st.ss[1]]
    };
    md.set("ss_threshold", &json!(255)).unwrap();
    md.set("ss_ratchet", &json!(false)).unwrap();
    assert_eq!(sim.state().ss[..2], [1, 255]);
    call(&t.s, "reset", json!({"id": md.id})).unwrap();
    assert_eq!(sim.state().ss[..2], before);
    // The MX-style simulator: SmartShift, gesture button → performance family.
    let snap = call(&t.s, "device", json!({"id": md.id})).unwrap();
    assert_eq!(snap["smartshift"]["enhanced"], json!(true));
    assert_eq!(snap["family"], json!("mouse-performance"));
}

#[test]
fn receiver_device_liveness() {
    let t = setup();
    let sim = SimulatedMouse::new("mx", 1);
    let ch = Channel::new(sim.clone());
    let md =
        t.s.attach_channel_device(&ch, 1, "sim://mx#1", sim.product_id)
            .unwrap();
    assert_eq!(md.connection(), "receiver");
    md.check_alive();
    assert!(md.online());
    sim.set_asleep(true); // powered off / switched to another host
    md.check_alive();
    md.check_alive();
    assert!(!md.online());
    sim.set_asleep(false);
    sim.state().dpi = 1000;
    md.check_alive(); // settings are applied again on wake-up
    assert!(md.online());
    ch.close();
}

#[test]
fn disconnect_marks_offline() {
    let t = setup();
    let md = t.s.device("b031-lift").unwrap();
    t.sim.disconnect();
    assert!(wait_for(|| !md.online(), 2.0));
    assert_eq!(
        call(
            &t.s,
            "set",
            json!({"id": "b031-lift", "key": "dpi", "value": 800})
        ),
        Err("The device is disconnected".into())
    );
    let list = call(&t.s, "devices", json!(null)).unwrap();
    assert_eq!(list[0]["online"], json!(false));
}

#[test]
fn errors_and_validation() {
    let t = setup();
    let id = json!("b031-lift");
    assert_eq!(
        call(&t.s, "device", json!({"id": "nope"})),
        Err("Device not found".into())
    );
    assert_eq!(
        call(&t.s, "device", json!([1])),
        Err("Invalid parameters".into())
    );
    assert_eq!(
        call(&t.s, "frobnicate", json!({"id": id})),
        Err("Unknown method: frobnicate".into())
    );
    assert_eq!(
        call(&t.s, "set", json!({"id": id, "key": "colour", "value": 1})),
        Err("Unknown setting: colour".into())
    );
    assert_eq!(
        call(&t.s, "set", json!({"id": id, "key": "dpi"})),
        Err("Invalid parameters".into())
    );
    assert_eq!(
        call(
            &t.s,
            "set",
            json!({"id": id, "key": "dpi", "value": "lots"})
        ),
        Err("Invalid parameters".into())
    );
    assert_eq!(
        call(
            &t.s,
            "set",
            json!({"id": id, "key": "ss_ratchet", "value": true})
        ),
        Err("This device doesn't support that setting".into())
    );
    assert_eq!(
        call(
            &t.s,
            "set_button",
            json!({"id": id, "cid": 0x99, "action": null})
        ),
        Err("Unknown control".into())
    );
    assert_eq!(
        call(
            &t.s,
            "set_button",
            json!({"id": id, "cid": 0x53, "action": {"type": "keys", "combo": "Ctrl+Nope"}})
        ),
        Err("Unknown key: Nope".into())
    );
    assert_eq!(
        call(
            &t.s,
            "set_button",
            json!({"id": id, "cid": 0x53, "action": "keys"})
        ),
        Err("Invalid parameters".into())
    );
    // Clamped to the nearest DPI the sensor has; returns what is stored.
    assert_eq!(
        call(&t.s, "set", json!({"id": id, "key": "dpi", "value": 1234})),
        Ok(json!(1200))
    );
    assert_eq!(
        call(
            &t.s,
            "set",
            json!({"id": id, "key": "dpi_presets", "value": [2400, 801, 2401, 9000]})
        ),
        Ok(json!([800, 2400, 4000]))
    );
    // The device refuses: the protocol's error name inside a translated sentence.
    let es =
        t.s.handle(
            "set_button",
            &json!({"id": id, "cid": 0x53, "action": {"type": "mouse", "button": "x"}}),
            Some("es"),
        )
        .unwrap_err();
    assert_eq!(es.0, "Botón desconocido");
    assert!(
        call(&t.s, "flow_status", json!({})).is_err(),
        "no Flow attached"
    );
}

#[test]
fn sim_api_and_battery_event() {
    let t = setup();
    let id = json!("b031-lift");
    call(
        &t.s,
        "set_button",
        json!({"id": id, "cid": 0x53, "action": {"type": "keys", "combo": "Ctrl+C"}}),
    )
    .unwrap();
    call(&t.s, "sim", json!({"id": id, "op": "press", "cid": 0x53})).unwrap();
    assert!(wait_for(|| key_events(&t.s).len() == 2, 2.0));
    let r = call(&t.s, "sim", json!({"id": id, "op": "release", "cid": 0x53})).unwrap();
    assert!(r["events"].is_array());
    call(
        &t.s,
        "sim",
        json!({"id": id, "op": "battery", "percent": 15, "charging": true}),
    )
    .unwrap();
    assert!(wait_for(
        || call(&t.s, "devices", json!(null)).unwrap()[0]["battery"]["percent"] == json!(15),
        2.0
    ));
    let b = &call(&t.s, "devices", json!(null)).unwrap()[0]["battery"];
    assert_eq!(
        (b["level"].clone(), b["charging"].clone()),
        (json!("low"), json!(true))
    );
}

#[test]
fn presets_api() {
    let t = setup();
    let p = call(&t.s, "presets", json!(null)).unwrap();
    assert_eq!(p["platform"], json!("linux"));
    let cats: Vec<&str> = p["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        cats,
        [
            "Mouse",
            "Editing",
            "Windows and desktops",
            "Browser",
            "Media",
            "System"
        ]
    );
    assert_eq!(p["default_gestures"]["tap"]["combo"], json!("Meta+W"));
    let es = t.s.handle("presets", &json!(null), Some("es")).unwrap();
    assert_eq!(es["categories"][0]["items"][3]["label"], json!("Atrás"));
}

#[test]
fn keyboard_profile() {
    let t = setup();
    t.s.add_simulated("keys");
    let snap = call(
        &t.s,
        "device",
        json!({"id": "b35b-wireless_keyboard_k-sim"}),
    )
    .unwrap();
    assert_eq!(snap["kind"], json!("keyboard"));
    assert_eq!(snap["dpi"], json!(null));
    assert_eq!(snap["controls"], json!([]));
    assert_eq!(snap["family"], json!("keyboard-full"));
    assert_eq!(snap["hosts"]["count"], json!(3));
}

#[test]
fn events_are_pushed() {
    use std::sync::Mutex;
    let dir = TmpDir::new("events");
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let s2 = seen.clone();
    let s = glisa_service::Service::new(glisa_service::ServiceOptions {
        config_path: Some(dir.path("c.json")),
        dry_run_input: true,
        ipc: Some(String::new()),
        events: Arc::new(move |e| s2.lock().unwrap().push(e.topic)),
        ..Default::default()
    })
    .unwrap();
    s.add_simulated("lift");
    assert!(seen.lock().unwrap().contains(&"devices".to_string()));
    seen.lock().unwrap().clear();
    s.handle(
        "set",
        &json!({"id": "b031-lift", "key": "dpi", "value": 800}),
        None,
    )
    .unwrap();
    assert_eq!(*seen.lock().unwrap(), ["device"]);
    s.stop();
}

#[test]
fn flow_host() {
    use glisa_common::{DeviceKind, FlowHost};
    let t = setup();
    let devs = t.s.flow_devices();
    assert_eq!(devs.len(), 1);
    assert_eq!(devs[0].name, "LIFT");
    assert_eq!(devs[0].key, "B031:LIFT");
    assert_eq!(devs[0].kind, DeviceKind::Mouse);
    assert_eq!((devs[0].hosts_count, devs[0].current_host), (3, 0));
    FlowHost::set_host(&*t.s, "b031-lift", 2).unwrap();
    assert!(wait_for(|| t.sim.state().host == 2, 2.0));
    assert!(FlowHost::set_host(&*t.s, "b031-lift", 9).is_err());
    t.s.release_all_input();
}
