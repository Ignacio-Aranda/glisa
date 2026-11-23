//! Rename (display name) and forget (remove a device) in the service.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use glisa_hidpp::f;
use glisa_hidpp::sim::SimulatedMouse;
use glisa_service::{Config, Service};
use serde_json::{Value, json};

struct Setup {
    dir: TmpDir,
    cfg: std::path::PathBuf,
    s: Arc<Service>,
    sim: Arc<SimulatedMouse>,
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.s.stop();
    }
}

fn setup() -> Setup {
    let dir = TmpDir::new("ident");
    let cfg = dir.path("c.json");
    let s = service(cfg.clone(), true);
    let sim = s.add_simulated("lift");
    Setup { dir, cfg, s, sim }
}

fn stored(cfg: &std::path::Path) -> Value {
    let v: Value = serde_json::from_slice(&std::fs::read(cfg).unwrap()).unwrap();
    v["devices"].clone()
}

#[test]
fn clean_display_name() {
    use glisa_common::clean_display_name as clean;
    assert_eq!(clean("  a \t b\n"), "a b");
    assert_eq!(clean("x\u{202E}evil\u{0}\u{1b}[31m"), "xevil[31m"); // no controls/bidi
    assert_eq!(clean("Ratón 🐭 \u{200D}"), "Ratón 🐭 \u{200D}".trim());
    assert_eq!(clean(""), "");
}

#[test]
fn rename_cleans_the_name_and_resets() {
    let t = setup();
    let id = json!("b031-lift");
    let snap = call(
        &t.s,
        "rename",
        json!({"id": id, "name": "  Office\t\tmouse \u{202E}\n "}),
    )
    .unwrap();
    assert_eq!(
        (snap["name"].clone(), snap["model"].clone()),
        (json!("Office mouse"), json!("LIFT"))
    );
    assert_eq!(
        stored(&t.cfg)["B031:LIFT"]["display_name"],
        json!("Office mouse")
    );
    let md = t.s.device("b031-lift").unwrap();
    assert_eq!(md.name, "LIFT"); // still the key, Flow and the log use it
    assert_eq!(md.key, "B031:LIFT");
    let full = call(&t.s, "device", json!({"id": id})).unwrap();
    assert_eq!(
        (full["name"].clone(), full["model"].clone()),
        (json!("Office mouse"), json!("LIFT"))
    );
    assert!(full["settings"].get("display_name").is_none());
    let list = call(&t.s, "devices", json!(null)).unwrap();
    assert_eq!(
        (list[0]["name"].clone(), list[0]["model"].clone()),
        (json!("Office mouse"), json!("LIFT"))
    );
    for reset in [
        json!(""),
        json!("   "),
        json!(null),
        json!("LIFT"),
        json!(" LIFT "),
    ] {
        call(&t.s, "rename", json!({"id": id, "name": "Something"})).unwrap();
        let r = call(&t.s, "rename", json!({"id": id, "name": reset})).unwrap();
        assert_eq!(r["name"], json!("LIFT"), "{reset}");
        assert!(
            stored(&t.cfg)
                .get("B031:LIFT")
                .and_then(|d| d.get("display_name"))
                .is_none()
        );
    }
    let forty = "x".repeat(40);
    assert_eq!(
        call(&t.s, "rename", json!({"id": id, "name": forty})).unwrap()["name"],
        json!(forty)
    );
    let e = call(&t.s, "rename", json!({"id": id, "name": "y".repeat(41)})).unwrap_err();
    assert!(e.contains("40"));
    // Errors in the client's language.
    let es = |params: Value| t.s.handle("rename", &params, Some("es")).unwrap_err().0;
    assert_eq!(
        es(json!({"id": id, "name": "y".repeat(41)})),
        "El nombre no puede tener más de 40 caracteres"
    );
    assert_eq!(
        es(json!({"id": id, "name": ["not", "text"]})),
        "El nombre tiene que ser texto"
    );
    assert_eq!(
        call(&t.s, "device", json!({"id": id})).unwrap()["name"],
        json!(forty)
    ); // unchanged
    // Forty characters, not bytes.
    let accents = "á".repeat(40);
    assert_eq!(
        call(&t.s, "rename", json!({"id": id, "name": accents})).unwrap()["name"],
        json!(accents)
    );
}

#[test]
fn rename_while_disconnected_and_reset_keeps_it() {
    let t = setup();
    let id = json!("b031-lift");
    call(&t.s, "rename", json!({"id": id, "name": "Desk"})).unwrap();
    t.s.device("b031-lift")
        .unwrap()
        .set("dpi", &json!(1600))
        .unwrap();
    call(&t.s, "reset", json!({"id": id})).unwrap(); // settings go, the name stays
    assert_eq!(
        stored(&t.cfg)["B031:LIFT"],
        json!({"buttons": {}, "display_name": "Desk"})
    );
    let md = t.s.device("b031-lift").unwrap();
    t.sim.disconnect();
    assert!(wait_for(|| !md.online(), 2.0));
    assert_eq!(
        call(&t.s, "rename", json!({"id": id, "name": "Travel"})).unwrap()["name"],
        json!("Travel")
    );
    assert!(call(&t.s, "set", json!({"id": id, "key": "dpi", "value": 800})).is_err()); // writes need the device
}

#[test]
fn forget_online_restores_factory_and_clears_everything() {
    let t = setup();
    let id = json!("b031-lift");
    let md = t.s.device("b031-lift").unwrap();
    md.set("dpi", &json!(3200)).unwrap();
    md.set("wheel_inverted", &json!(true)).unwrap();
    md.set_button(0x53, Some(&json!({"type": "keys", "combo": "Ctrl+C"})))
        .unwrap();
    call(&t.s, "rename", json!({"id": id, "name": "Desk"})).unwrap();
    assert_eq!(t.sim.state().reporting[&0x53].0 & 1, 1);
    assert_eq!(call(&t.s, "forget", json!({"id": id})), Ok(json!(true)));
    assert_eq!(t.sim.state().dpi, 1000); // factory behaviour on the device
    assert_eq!(t.sim.state().wheel_mode & 0x04, 0);
    assert_eq!(t.sim.state().reporting[&0x53], (0, 0));
    assert!(stored(&t.cfg).get("B031:LIFT").is_none()); // settings and name, on disk too
    assert!(
        Config::new(Some(t.cfg.clone())).snapshot()["devices"]
            .get("B031:LIFT")
            .is_none()
    );
    assert_eq!(call(&t.s, "devices", json!(null)), Ok(json!([])));
    assert!(call(&t.s, "device", json!({"id": id})).is_err());
    // Still connected: it does not come back by itself…
    assert_eq!(t.s.forgotten(), vec!["sim://lift".to_string()]);
    t.sim.press(0x53); // its events drive nothing any more
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(call(&t.s, "devices", json!(null)), Ok(json!([])));
    assert!(key_events(&t.s).is_empty());
    // …only when it reconnects, with default settings.
    t.sim.notify(f::WIRELESS_DEVICE_STATUS, 0, &[1]);
    assert!(wait_for(|| t.s.device("b031-lift").is_some(), 3.0));
    let back = call(&t.s, "device", json!({"id": id})).unwrap();
    assert_eq!(
        (
            back["name"].clone(),
            back["settings"].clone(),
            back["dpi"]["current"].clone()
        ),
        (json!("LIFT"), json!({}), json!(1000))
    );
    assert!(t.s.forgotten().is_empty());
    assert!(
        back["controls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["action"] == json!({"type": "default"}))
    );
}

#[test]
fn forget_offline_device() {
    let t = setup();
    let md = t.s.device("b031-lift").unwrap();
    md.set("dpi", &json!(1600)).unwrap();
    t.sim.disconnect();
    assert!(wait_for(|| !md.online(), 2.0));
    assert_eq!(
        call(&t.s, "forget", json!({"id": "b031-lift"})),
        Ok(json!(true))
    );
    assert_eq!(call(&t.s, "devices", json!(null)), Ok(json!([])));
    assert!(stored(&t.cfg).get("B031:LIFT").is_none());
    assert!(t.s.forgotten().is_empty()); // nothing left watching a closed node
    t.s.attach_sim_endpoint(&t.sim.connect()).unwrap(); // it connects again: back, as new
    let snap = call(&t.s, "device", json!({"id": "b031-lift"})).unwrap();
    assert_eq!(
        (snap["online"].clone(), snap["settings"].clone()),
        (json!(true), json!({}))
    );
    let _ = &t.dir;
}

#[test]
fn forget_bluetooth_device_comes_back_only_with_a_new_node() {
    let dir = TmpDir::new("bt");
    let s = service(dir.path("bt.json"), true);
    let hid = Arc::new(FakeHid::default());
    s.set_hid_backend(hid.clone());
    let n = node("/dev/hidraw-test0", 0x05, 0xB031, "LIFT");
    let sims: Arc<std::sync::Mutex<Vec<Arc<SimulatedMouse>>>> = Arc::default();
    let sims2 = sims.clone();
    hid.on_open(&n.path, move || {
        let sim = SimulatedMouse::new("lift", 0xFF);
        sims2.lock().unwrap().push(sim.clone());
        Box::new(SimHandle(sim))
    });
    hid.set_nodes(vec![n.clone()]);
    s.scan_once();
    assert!(wait_for(|| s.device("b031-lift").is_some(), 3.0));
    let md = s.device("b031-lift").unwrap();
    assert_eq!(
        (md.source.as_str(), md.connection()),
        (n.path.as_str(), "bluetooth")
    );
    md.set("dpi", &json!(2000)).unwrap();
    call(&s, "forget", json!({"id": md.id})).unwrap();
    assert_eq!(sims.lock().unwrap()[0].state().dpi, 1000);
    let ch = s.channel(&n.path).unwrap(); // its node is still here…
    for _ in 0..3 {
        s.scan_once();
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(s.device("b031-lift").is_none()); // …so it is not probed again
    assert_eq!(*hid.made.lock().unwrap(), vec![n.path.clone()]);
    assert!(!ch.is_closed());
    // The node disappears: its channel and reader go, nothing is left behind.
    hid.set_nodes(vec![]);
    s.scan_once();
    assert!(s.channel(&n.path).is_none());
    assert!(ch.is_closed());
    assert!(ch.join_reader(Duration::from_secs(2)));
    assert!(s.forgotten().is_empty());
    // And comes back: a new device, with default settings.
    hid.set_nodes(vec![n.clone()]);
    s.scan_once();
    assert!(wait_for(|| s.device("b031-lift").is_some(), 3.0));
    let snap = call(&s, "device", json!({"id": "b031-lift"})).unwrap();
    assert_eq!(
        (
            snap["settings"].clone(),
            snap["dpi"]["current"].clone(),
            snap["online"].clone()
        ),
        (json!({}), json!(1000), json!(true))
    );
    assert_eq!(s.channel_paths(), vec![n.path.clone()]); // only the new connection reads
    let new_ch = s.channel(&n.path).unwrap();
    assert!(!Arc::ptr_eq(&new_ch, &ch));
    assert!(new_ch.reader_alive());
    assert!(!ch.reader_alive());
    s.stop();
}

#[test]
fn forget_device_behind_a_receiver() {
    let dir = TmpDir::new("rx");
    let s = service(dir.path("rx.json"), true);
    let hid = Arc::new(FakeHid::default());
    s.set_hid_backend(hid.clone());
    let sim = SimulatedMouse::new("mx", 2);
    let n = node("/dev/hidraw-recv", 0x03, 0xC52B, "Receiver");
    assert!(n.is_receiver());
    let sim2 = sim.clone();
    let path = n.path.clone();
    hid.on_open(&n.path, move || {
        Box::new(FakeReceiver::new(&path, vec![(2, sim2.clone())]))
    });
    hid.set_nodes(vec![n.clone()]);
    s.scan_once();
    let source = format!("{}#2", n.path);
    assert!(wait_for(
        || s.devices().iter().any(|m| m.source == source),
        5.0
    ));
    let md = s
        .devices()
        .into_iter()
        .find(|m| m.source == source)
        .unwrap();
    assert_eq!(md.connection(), "receiver");
    assert_eq!(md.key, "C52B:Wireless Mouse MX-Sim"); // the receiver's product id, as before
    md.set_button(0xC3, Some(&json!({"type": "keys", "combo": "Ctrl+V"})))
        .unwrap();
    call(&s, "rename", json!({"id": md.id, "name": "Couch"})).unwrap();
    call(&s, "forget", json!({"id": md.id})).unwrap();
    assert_eq!(sim.state().reporting[&0xC3], (0, 0));
    assert_eq!(s.forgotten(), vec![source.clone()]);
    for _ in 0..2 {
        // still answering: still removed
        s.scan_once();
        s.check_forgotten(Duration::ZERO);
    }
    assert!(s.device(&md.id).is_none());
    sim.set_asleep(true); // switched off…
    s.check_forgotten(Duration::ZERO);
    s.check_forgotten(Duration::ZERO);
    assert!(s.device(&md.id).is_none());
    sim.set_asleep(false); // …and on again: it reconnected
    s.check_forgotten(Duration::ZERO);
    let again = s.device(&md.id).expect("added again");
    let snap = again.snapshot(true);
    assert_eq!(
        (
            snap["name"].clone(),
            snap["connection"].clone(),
            snap["settings"].clone()
        ),
        (json!("Wireless Mouse MX-Sim"), json!("receiver"), json!({}))
    );
    assert!(s.forgotten().is_empty());
    // Removed again, then it announces its reconnection itself.
    call(&s, "forget", json!({"id": md.id})).unwrap();
    sim.notify(f::WIRELESS_DEVICE_STATUS, 0, &[1]);
    assert!(wait_for(|| s.device(&md.id).is_some(), 3.0));
    call(&s, "forget", json!({"id": md.id})).unwrap();
    assert_eq!(*hid.made.lock().unwrap(), vec![n.path.clone()]); // never opened twice
    let ch = s.channel(&n.path).unwrap();
    hid.set_nodes(vec![]); // receiver unplugged
    s.scan_once();
    assert!(ch.is_closed());
    assert!(s.forgotten().is_empty());
    s.stop();
}

#[test]
fn denied_nodes_are_reported() {
    let dir = TmpDir::new("denied");
    let s = service(dir.path("c.json"), false);
    let hid = Arc::new(FakeHid::default());
    s.set_hid_backend(hid.clone());
    hid.set_nodes(vec![node("/dev/hidraw-x", 0x05, 0xB031, "LIFT")]); // no opener: EACCES
    s.scan_once();
    let st = call(&s, "status", json!(null)).unwrap();
    assert_eq!(st["denied"], json!(["/dev/hidraw-x"]));
    if cfg!(target_os = "linux") {
        assert_eq!(st["permissions"][0]["id"], json!("hid"));
        assert_eq!(st["permissions"][0]["state"], json!("missing"));
    }
    s.stop();
}
