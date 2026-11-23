//! Flow between two (or more) Rust computers on loopback: pairing, discovery, crossing,
//! clipboard, routed networks, arrival timing, the API.

mod common;

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use common::*;
use glisa_common::{AppearingDevice, DeviceKind, FlowApi};
use glisa_flow::{CLIPBOARD_MAX, free_ports};
use serde_json::json;

// ================================================================== pairing
#[test]
fn discovery_and_matching_sas() {
    let (a, b) = two_computers();
    assert!(wait_until(5.0, || a.call("flow_peers", json!({})).unwrap()
        ["discovered"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["id"] == b.id().as_str())));
    b.call("flow_pair_start", json!({})).unwrap();
    a.call("flow_pair_request", json!({"peer": b.id()}))
        .unwrap();
    assert!(wait_until(5.0, || a.pairing_state() == "confirm"
        && b.pairing_state() == "confirm"));
    let sa = a.call("flow_status", json!({})).unwrap()["pairing"].clone();
    let sb = b.call("flow_status", json!({})).unwrap()["pairing"].clone();
    assert_eq!(sa["sas"], sb["sas"]);
    let code = sa["sas"].as_str().unwrap();
    assert!(code.len() == 6 && code.bytes().all(|c| c.is_ascii_digit()));
    assert_eq!(sb["peer"]["name"], "equipo-a");
    a.call("flow_pair_confirm", json!({"accept": true}))
        .unwrap();
    sleep(0.3);
    assert!(a.flow.store.peers().is_empty()); // one side alone is not enough
    assert!(wait_until(5.0, || b.pairing()["remote"] == true));
    b.call("flow_pair_confirm", json!({"accept": true}))
        .unwrap();
    assert!(wait_until(5.0, || a.pairing_state() == "done"
        && b.pairing_state() == "done"));
    assert_eq!(a.flow.store.peer(&b.id()).unwrap().pin, b.flow.store.pin());
    assert_eq!(b.flow.store.peer(&a.id()).unwrap().pin, a.flow.store.pin());
    assert!(linked(&a, &b, 6.0));
    // pairing closed: a replayed introduction is not even listened to
    assert!(wait_until(3.0, || std::net::TcpStream::connect((
        "127.0.0.1",
        b.port + 1
    ))
    .is_err()));
}

#[test]
fn user_says_numbers_differ() {
    let (a, b) = two_computers();
    pair_with(&a, &b, true, false);
    assert!(a.flow.store.peers().is_empty());
    assert!(b.flow.store.peers().is_empty());
    assert_eq!(a.pairing()["reason"], "rejected");
}

#[test]
fn not_pairing_and_expiry() {
    let (a, b) = two_computers();
    a.call(
        "flow_pair_request",
        json!({"address": format!("127.0.0.1:{}", b.port)}),
    )
    .unwrap();
    assert!(wait_until(5.0, || a.pairing_state() == "failed"));
    assert_eq!(a.pairing()["reason"], "not_pairing");
    b.flow.set_pairing_seconds(0.4);
    b.call("flow_pair_start", json!({})).unwrap();
    sleep(0.6);
    assert_eq!(
        b.call("flow_status", json!({})).unwrap()["pairing"]["state"],
        "expired"
    );
    assert!(wait_until(3.0, || !b
        .flow
        .pairing()
        .unwrap()
        .has_listener()));
    a.flow.pair_cancel();
    a.call(
        "flow_pair_request",
        json!({"address": format!("127.0.0.1:{}", b.port)}),
    )
    .unwrap();
    assert!(wait_until(5.0, || a.pairing_state() == "failed"));
    assert!(b.flow.store.peers().is_empty());
}

#[test]
fn one_attempt_at_a_time_and_rate_limit() {
    let (a, b) = two_computers();
    let c = Computer::new("equipo-c", Opts::default()).enable();
    b.call("flow_pair_start", json!({})).unwrap();
    a.call(
        "flow_pair_request",
        json!({"address": format!("127.0.0.1:{}", b.port)}),
    )
    .unwrap();
    assert!(wait_until(5.0, || b.pairing_state() == "confirm"));
    c.call(
        "flow_pair_request",
        json!({"address": format!("127.0.0.1:{}", b.port)}),
    )
    .unwrap();
    assert!(wait_until(5.0, || c.pairing_state() == "failed"));
    assert_eq!(c.pairing()["reason"], "busy");
    b.call("flow_pair_cancel", json!({})).unwrap();
    assert!(wait_until(5.0, || a.pairing_state() == "failed"));
    // attempts are limited per pairing window
    b.call("flow_pair_start", json!({})).unwrap();
    b.flow.pairing().unwrap().set_attempts(3);
    c.flow.pair_cancel();
    c.call(
        "flow_pair_request",
        json!({"address": format!("127.0.0.1:{}", b.port)}),
    )
    .unwrap();
    assert!(wait_until(5.0, || c.pairing_state() == "failed"));
    assert_eq!(c.pairing()["reason"], "rate_limited");
}

#[test]
fn mitm_relay_is_caught_by_the_sas() {
    // M sits between a and b with its own certificate: it answers a as if it were b and
    // pairs with b as if it were a. The commitments make both SAS independent.
    let (a, b) = two_computers();
    let m1 = Computer::new("equipo-b", Opts::default()); // faces a, pretending to be b
    let m2 = Computer::new("equipo-a", Opts::default()); // faces b, pretending to be a
    m1.flow.store.ensure_identity().unwrap();
    m2.flow.store.ensure_identity().unwrap();
    for f in ["cert.pem", "key.pem"] {
        std::fs::copy(m1.flow.store.dir.join(f), m2.flow.store.dir.join(f)).unwrap();
    }
    m2.flow.store.reset_pin_cache();
    let (m1, m2) = (m1.enable(), m2.enable());
    m1.call("flow_pair_start", json!({})).unwrap();
    b.call("flow_pair_start", json!({})).unwrap();
    a.call(
        "flow_pair_request",
        json!({"address": format!("127.0.0.1:{}", m1.port)}),
    )
    .unwrap();
    m2.call(
        "flow_pair_request",
        json!({"address": format!("127.0.0.1:{}", b.port)}),
    )
    .unwrap();
    for x in [&a, &b, &m1, &m2] {
        assert!(
            wait_until(8.0, || x.pairing_state() == "confirm"),
            "{}",
            x.name
        );
    }
    assert_eq!(a.pairing()["peer"]["name"], "equipo-b"); // names can be faked…
    assert_ne!(a.pairing()["sas"], b.pairing()["sas"]); // …the SAS cannot
    a.call("flow_pair_confirm", json!({"accept": false}))
        .unwrap();
    b.call("flow_pair_confirm", json!({"accept": false}))
        .unwrap();
    m1.call("flow_pair_confirm", json!({"accept": true}))
        .unwrap();
    m2.call("flow_pair_confirm", json!({"accept": true}))
        .unwrap();
    assert!(wait_until(5.0, || a.pairing_state() == "failed"
        && b.pairing_state() == "failed"));
    assert!(a.flow.store.peers().is_empty());
    assert!(b.flow.store.peers().is_empty());
}

// ================================================================== switching
fn switching() -> (Computer, Computer, std::sync::Arc<SimHub>) {
    let (a, b) = paired_pair();
    let hub = shared_device(&a, &b);
    (a, b, hub)
}

#[test]
fn right_then_back_with_entry_points() {
    let (a, b, hub) = switching();
    a.backend.hit("right", 0.25);
    assert!(
        wait_until(5.0, || hub.moves() == vec![1]),
        "{:?} {:?}",
        hub.moves(),
        a.events()
    );
    assert!(wait_until(5.0, || !b.backend.placements().is_empty()));
    assert_eq!(b.backend.placements(), vec![(6, 360)]); // 2560×1440, left edge
    let ev = b.backend.pointer.events.lock().unwrap().clone();
    assert_eq!(ev[ev.len() - 3..ev.len() - 1], [(3, 0, 166), (3, 1, 16406)]);
    assert!(!a.device(LIFT).unwrap().online);
    assert!(b.device(LIFT).unwrap().online);
    assert_eq!(
        b.call("flow_status", json!({})).unwrap()["last_arrival"]["side"],
        "left"
    );
    sleep(0.9); // cooldown
    b.backend.hit("left", 0.75);
    assert!(wait_until(5.0, || hub.moves() == vec![1, 0]));
    assert!(wait_until(5.0, || !a.backend.placements().is_empty()));
    assert_eq!(a.backend.placements(), vec![(1913, 809)]);
    assert!(wait_until(5.0, || a.device(LIFT).is_some_and(|d| d.online)));
    assert!(a.host.released.load(std::sync::atomic::Ordering::SeqCst) >= 1); // held keys released first
}

#[test]
fn clipboard_follows() {
    let (a, b, _hub) = switching();
    a.backend.set_clip("hola, ñandú");
    a.backend.hit("right", 0.5);
    assert!(wait_until(5.0, || b.backend.clip().as_deref() == Some("hola, ñandú")));
}

#[test]
fn clipboard_opt_out_and_size_cap() {
    let (a, b, _hub) = switching();
    b.call("flow_set_options", json!({"clipboard": false}))
        .unwrap();
    a.backend.set_clip("secreto");
    a.backend.hit("right", 0.5);
    assert!(wait_until(5.0, || !b.backend.placements().is_empty()));
    sleep(0.3);
    assert_eq!(b.backend.clip().as_deref(), Some(""));
    b.call("flow_set_options", json!({"clipboard": true}))
        .unwrap();
    sleep(0.9);
    b.backend.set_clip(&"x".repeat(CLIPBOARD_MAX + 1));
    b.backend.hit("left", 0.5);
    assert!(wait_until(5.0, || !a.backend.placements().is_empty()));
    assert!(
        wait_until(5.0, || b
            .events_in("es")
            .iter()
            .any(|t| t.contains("demasiado grande"))),
        "{:?}",
        b.events_in("es")
    );
    assert_eq!(a.backend.clip().as_deref(), Some("secreto"));
}

#[test]
fn unreachable_peer_keeps_the_mouse() {
    let (a, b, hub) = switching();
    b.flow.stop();
    assert!(wait_until(15.0, || a.peer_state(&b)["online"] == false));
    a.backend.hit("right", 0.5);
    assert!(wait_until(5.0, || a
        .events_in("es")
        .iter()
        .any(|t| t.contains("No se puede contactar con equipo-b"))));
    sleep(0.3);
    assert!(hub.moves().is_empty());
    assert!(a.device(LIFT).unwrap().online);
}

#[test]
fn disabled_peer_refuses() {
    let (a, b, hub) = switching();
    b.flow.set_running_flag(false); // the server still answers but Flow is off there
    a.backend.hit("right", 0.5);
    assert!(
        wait_until(5.0, || a
            .events_in("es")
            .iter()
            .any(|t| t.contains("desactivado"))),
        "{:?}",
        a.events_in("es")
    );
    assert!(hub.moves().is_empty());
    b.flow.set_running_flag(true);
}

#[test]
fn cooldown_and_hysteresis() {
    let (a, b, hub) = switching();
    b.call("flow_set_options", json!({"cooldown_ms": 1500}))
        .unwrap();
    a.backend.hit("right", 0.5);
    assert!(wait_until(5.0, || !b.backend.placements().is_empty()));
    b.backend.hit("left", 0.5); // right away: ignored
    sleep(0.4);
    assert_eq!(hub.moves(), vec![1]);
    sleep(1.2);
    b.backend.hit("left", 0.6); // moving along the edge after the cooldown
    assert!(wait_until(5.0, || hub.moves() == vec![1, 0]));
}

#[test]
fn refused_attempt_needs_leaving_the_edge() {
    let (a, b, hub) = switching();
    a.flow.link(&b.id()).unwrap().close();
    b.flow.stop();
    a.backend.hit("right", 0.5);
    assert!(wait_until(5.0, || a
        .events()
        .iter()
        .any(|t| t.contains("Can't reach"))));
    b.flow.start_now().unwrap();
    assert!(linked(&a, &b, 10.0));
    a.backend.hit("right", 0.6); // still at the edge: disarmed
    sleep(0.4);
    assert!(hub.moves().is_empty());
    a.backend.away(); // leave and come back
    a.backend.hit("right", 0.6);
    assert!(wait_until(5.0, || hub.moves() == vec![1]));
}

#[test]
fn corner_dead_zone() {
    let (a, _b, hub) = switching();
    a.backend.hit("right", 0.01);
    sleep(0.4);
    assert!(hub.moves().is_empty());
    a.backend.hit("right", 0.3); // slide out of the corner
    assert!(wait_until(5.0, || hub.moves() == vec![1]));
}

#[test]
fn ctrl_only() {
    let (a, _b, hub) = switching();
    a.call("flow_set_options", json!({"require_ctrl": true}))
        .unwrap();
    a.backend.hit("right", 0.5);
    sleep(0.4);
    assert!(hub.moves().is_empty());
    a.backend.set_ctrl(true); // pressing Ctrl at the edge switches
    assert!(wait_until(5.0, || hub.moves() == vec![1]));
}

#[test]
fn ctrl_only_without_ctrl_support() {
    let (a, _b, hub) = switching();
    a.backend
        .ctrl_supported
        .store(false, std::sync::atomic::Ordering::SeqCst);
    a.call("flow_set_options", json!({"require_ctrl": true}))
        .unwrap();
    assert_eq!(
        a.call("flow_status", json!({})).unwrap()["ctrl_available"],
        false
    );
    a.backend.hit("right", 0.5);
    assert!(wait_until(5.0, || a
        .events()
        .iter()
        .any(|t| t.contains("Ctrl"))));
    assert!(hub.moves().is_empty());
}

#[test]
fn no_crossing_while_a_button_is_held() {
    let (a, _b, hub) = switching();
    *a.backend.buttons.lock().unwrap() = Some(true); // dragging a window or a file to the edge
    a.backend.hit("right", 0.5);
    sleep(0.4);
    assert!(hub.moves().is_empty());
    *a.backend.buttons.lock().unwrap() = Some(false); // released at the edge: still nothing…
    sleep(0.2);
    assert!(hub.moves().is_empty());
    a.backend.hit("right", 0.55); // …until the pointer pushes the edge again
    assert!(wait_until(5.0, || hub.moves() == vec![1]));
}

#[test]
fn unknown_button_state_still_switches() {
    let (a, _b, hub) = switching();
    *a.backend.buttons.lock().unwrap() = None; // a desktop that can't tell
    a.backend.hit("right", 0.5);
    assert!(wait_until(5.0, || hub.moves() == vec![1]));
}

#[test]
fn no_channel_known() {
    let (a, b, hub) = switching();
    let mut d = a.device(LIFT).unwrap();
    d.host_entries = vec![
        json!({"index": 0, "paired": true, "name": "equipo-a"}),
        json!({"index": 1, "paired": true, "name": "Mac"}),
        json!({"index": 2, "paired": false, "name": ""}),
    ];
    a.host.set_device(d);
    let st = a.call("flow_status", json!({})).unwrap();
    assert!(st["devices"][0]["channels"][b.id()]["suggested"].is_null());
    a.backend.hit("right", 0.5);
    assert!(wait_until(5.0, || a
        .events_in("es")
        .iter()
        .any(|t| t.contains("canal"))));
    assert!(hub.moves().is_empty());
    a.call(
        "flow_set_channel",
        json!({"device": LIFT, "peer": b.id(), "channel": 1}),
    )
    .unwrap();
    assert_eq!(
        a.call("flow_status", json!({})).unwrap()["devices"][0]["channels"][b.id()]["channel"],
        1
    );
    a.backend.away();
    a.backend.hit("right", 0.5);
    assert!(wait_until(5.0, || hub.moves() == vec![1]));
}

#[test]
fn edge_while_the_mouse_is_away_is_silent() {
    let (a, _b, hub) = switching();
    a.backend.hit("right", 0.5);
    assert!(wait_until(5.0, || hub.moves() == vec![1]
        && !a.device(LIFT).unwrap().online));
    sleep(0.9);
    let seq = a.call("flow_events", json!({"since": 0})).unwrap()["seq"]
        .as_u64()
        .unwrap();
    a.backend.away(); // e.g. a's touchpad pushes the edge again
    a.backend.hit("right", 0.4);
    sleep(0.4);
    assert_eq!(
        a.call("flow_events", json!({"since": seq})).unwrap()["events"],
        json!([])
    );
    assert_eq!(hub.moves(), vec![1]);
}

#[test]
fn keyboard_follows() {
    let (a, b, hub) = switching();
    let khub = shared_device_kind(&a, &b, KEYS, "Keys", DeviceKind::Keyboard);
    a.backend.hit("right", 0.5);
    assert!(wait_until(5.0, || hub.moves() == vec![1] && khub.moves() == vec![1]));
    assert!(wait_until(5.0, || b.device(KEYS).is_some_and(|d| d.online)));
    sleep(0.9);
    b.call("flow_set_options", json!({"keyboard": false}))
        .unwrap();
    b.backend.hit("left", 0.5);
    assert!(wait_until(5.0, || hub.moves() == vec![1, 0]));
    sleep(0.3);
    assert_eq!(khub.moves(), vec![1]); // the keyboard stayed on b
}

// ================================================================== arrival timing
fn arrival() -> Computer {
    let b = Computer::new("equipo-b", Opts::default()).enable();
    assert!(b.flow.expect_arrival_for_tests("right", 0.5, &["LIFT"]));
    b
}

fn node(receiver: bool) -> AppearingDevice {
    AppearingDevice {
        path: "/dev/hidraw9".into(),
        vendor: 0x046d,
        product: 0xb031,
        is_receiver: receiver,
        is_bluetooth: !receiver,
        appeared_ms: glisa_common::monotonic_ms(),
    }
}

fn lift() -> glisa_common::FlowDevice {
    glisa_common::FlowDevice {
        id: LIFT.into(),
        key: "B031:LIFT".into(),
        name: "LIFT".into(),
        kind: DeviceKind::Mouse,
        online: true,
        hosts_count: 3,
        current_host: 1,
        host_entries: vec![],
    }
}

#[test]
fn pointer_placed_when_the_node_appears() {
    let b = arrival();
    b.flow.device_appearing(&node(false));
    assert!(wait_until(3.0, || !b.backend.placements().is_empty()));
    assert_eq!(b.backend.placements().len(), 1);
    assert!(!b.flow.has_pending_arrival());
}

#[test]
fn slow_hidpp_setup_does_not_place_again() {
    let b = arrival();
    b.flow.device_appearing(&node(false));
    assert!(wait_until(3.0, || !b.backend.placements().is_empty()));
    b.flow
        .arrived(&lift(), Some(Instant::now() - Duration::from_millis(3500))); // registration seconds later
    b.host.set_device(lift());
    b.flow.device_online(LIFT);
    sleep(0.3);
    assert_eq!(b.backend.placements().len(), 1);
}

#[test]
fn late_arrival_never_moves_the_pointer() {
    let b = arrival();
    b.flow
        .arrived(&lift(), Some(Instant::now() - Duration::from_millis(3500)));
    assert!(b.backend.placements().is_empty());
    assert!(!b.flow.has_pending_arrival());
    assert_eq!(
        b.call("flow_status", json!({})).unwrap()["last_arrival"]["late"],
        true
    );
}

#[test]
fn fresh_registration_still_places() {
    let b = arrival();
    b.flow
        .arrived(&lift(), Some(Instant::now() - Duration::from_millis(200)));
    assert_eq!(b.backend.placements().len(), 1);
    let b = arrival();
    b.host.set_device(lift());
    b.flow.device_online(LIFT); // a receiver device answering again: fresh
    assert!(wait_until(3.0, || b.backend.placements().len() == 1));
}

#[test]
fn receiver_nodes_do_not_count() {
    let b = arrival();
    b.flow.device_appearing(&node(true));
    sleep(0.3);
    assert!(b.backend.placements().is_empty());
    assert!(b.flow.has_pending_arrival());
}

// ================================================================== API
#[test]
fn status_layout_options() {
    let (a, b) = paired_pair();
    let st = a.call("flow_status", json!({})).unwrap();
    for key in [
        "enabled",
        "backend",
        "available",
        "ctrl_available",
        "ports",
        "peers",
        "pairing",
        "devices",
        "options",
        "fingerprint",
        "firewall",
        "transfers",
        "staging",
        "screens",
        "clipboard_backend",
        "files_backend",
    ] {
        assert!(st.get(key).is_some(), "{key}");
    }
    assert_eq!(st["peers"][0]["side"], "right");
    assert_eq!(st["peers"][0]["online"], true);
    assert_eq!(
        st["ports"]["pairing"].as_u64().unwrap(),
        st["ports"]["tcp"].as_u64().unwrap() + 1
    );
    a.call("flow_set_layout", json!({"peer": b.id(), "side": "top"}))
        .unwrap();
    assert_eq!(a.flow.store.peer_at("top"), Some(b.id()));
    assert!(
        a.call(
            "flow_set_layout",
            json!({"peer": b.id(), "side": "diagonal"})
        )
        .is_err()
    );
    assert!(
        a.call("flow_set_options", json!({"require_ctrl": "sí"}))
            .is_err()
    );
    assert_eq!(
        a.call("flow_set_options", json!({"cooldown_ms": 10}))
            .unwrap()["cooldown_ms"],
        200
    );
    a.call("flow_enable", json!({"enabled": false})).unwrap();
    assert_eq!(a.call("flow_status", json!({})).unwrap()["running"], false);
    a.call("flow_enable", json!({"enabled": true})).unwrap();
    assert_eq!(a.call("flow_status", json!({})).unwrap()["running"], true);
    assert_eq!(
        a.call_in(Some("en"), "flow_nope", json!({})).unwrap_err(),
        "Unknown method: flow_nope"
    );
    assert_eq!(
        a.call_in(Some("en"), "flow_simulate", json!({"op": "nope"}))
            .unwrap_err(),
        "Unknown operation"
    );
    assert_eq!(
        a.call_in(Some("en"), "flow_enable", json!({})).unwrap_err(),
        "Missing enabled (true or false)"
    );
}

#[test]
fn peer_address_api() {
    let (a, b) = paired_pair();
    let p = a.call("flow_peers", json!({})).unwrap()["peers"][0].clone();
    for key in [
        "address",
        "address_source",
        "manual_address",
        "learned_address",
        "last_error",
        "state",
        "offline_for",
    ] {
        assert!(p.get(key).is_some(), "{key}");
    }
    assert_eq!(
        (
            p["address"].as_str().unwrap(),
            p["address_source"].as_str().unwrap(),
            p["manual_address"].as_str().unwrap()
        ),
        (format!("127.0.0.1:{}", b.port).as_str(), "learned", "")
    );
    for (bad, msg) in [
        (
            json!("bad host!"),
            "no es un nombre de equipo ni una dirección IP válidos",
        ),
        (
            json!("127.0.0.1:0"),
            "El puerto debe ser un número del 1 al 65535",
        ),
        (json!(5), "Dirección no válida"),
    ] {
        let e = a
            .call(
                "flow_set_peer_address",
                json!({"peer": b.id(), "address": bad}),
            )
            .unwrap_err();
        assert!(e.contains(msg), "{e}");
    }
    let e = a
        .call(
            "flow_set_peer_address",
            json!({"peer": "fedcba9876543210", "address": "10.0.0.1"}),
        )
        .unwrap_err();
    assert!(e.contains("no está vinculado"), "{e}");
    assert!(a.flow.store.peer(&b.id()).unwrap().manual_address.is_none());
}

// ================================================================== routed networks
fn routed(broadcast: bool, discovery: bool) -> (Computer, Computer) {
    let (a, b) = two_computers_with(broadcast, discovery);
    pair(&a, &b);
    assert!(linked(&a, &b, 6.0));
    (a, b)
}

/// Stop both, store these addresses, start both again (new links: no discovery hints).
fn restart_with(
    a: &Computer,
    b: &Computer,
    a_knows_b: Option<(String, u16)>,
    b_knows_a: Option<(String, u16)>,
) {
    a.flow.stop();
    b.flow.stop();
    if let Some((h, p)) = a_knows_b {
        a.flow.store.set_address(&b.id(), &h, p);
    }
    if let Some((h, p)) = b_knows_a {
        b.flow.store.set_address(&a.id(), &h, p);
    }
    a.flow.start_now().unwrap();
    b.flow.start_now().unwrap();
}

#[test]
fn pairing_stores_the_address_on_both_sides() {
    let (a, b) = routed(true, false);
    assert_eq!(
        a.flow.store.peer(&b.id()).unwrap().address,
        Some(("127.0.0.1".into(), b.port))
    );
    assert_eq!(
        b.flow.store.peer(&a.id()).unwrap().address,
        Some(("127.0.0.1".into(), a.port))
    );
}

#[test]
fn manual_address_beats_a_stale_one() {
    let (a, b) = routed(true, false);
    let nowhere = (
        "127.0.0.1".to_string(),
        free_ports(Ipv4Addr::LOCALHOST).unwrap(),
    );
    restart_with(&a, &b, Some(nowhere.clone()), Some(nowhere.clone()));
    assert!(!linked(&a, &b, 1.5));
    let p = a.peer_state(&b);
    assert_eq!(
        (
            p["state"].as_str().unwrap(),
            p["address"].as_str().unwrap(),
            p["address_source"].as_str().unwrap()
        ),
        (
            "offline",
            format!("127.0.0.1:{}", nowhere.1).as_str(),
            "learned"
        )
    );
    assert!(!p["last_error"].as_str().unwrap().is_empty());
    let p = a
        .call(
            "flow_set_peer_address",
            json!({"peer": b.id(), "address": format!(" 127.0.0.1:{} ", b.port)}),
        )
        .unwrap();
    assert_eq!(p["manual_address"], format!("127.0.0.1:{}", b.port));
    assert!(linked(&a, &b, 4.0)); // and b learnt a's address from a's hello
    let p = a.peer_state(&b);
    assert_eq!(p["address"], format!("127.0.0.1:{}", b.port));
    assert_eq!(
        (p["address_source"].as_str(), p["last_error"].as_str()),
        (Some("manual"), Some(""))
    );
    assert_eq!(
        b.flow.store.peer(&a.id()).unwrap().address,
        Some(("127.0.0.1".into(), a.port))
    );
    let p = a
        .call(
            "flow_set_peer_address",
            json!({"peer": b.id(), "address": "auto"}),
        )
        .unwrap();
    assert_eq!(
        (p["manual_address"].as_str(), p["address_source"].as_str()),
        (Some(""), Some("learned"))
    );
    assert!(a.flow.store.peer(&b.id()).unwrap().manual_address.is_none());
}

#[test]
fn address_learnt_from_the_incoming_hello() {
    // no announcements at all: a can only learn b's address from b's own connection
    let (a, b) = routed(true, false);
    assert!(a.flow.discovery_destinations().is_none());
    restart_with(
        &a,
        &b,
        Some(("127.0.0.1".into(), free_ports(Ipv4Addr::LOCALHOST).unwrap())),
        None,
    );
    assert!(linked(&a, &b, 6.0));
    assert_eq!(
        a.flow.store.peer(&b.id()).unwrap().address,
        Some(("127.0.0.1".into(), b.port))
    ); // the advertised port
    assert_eq!(
        a.flow.link(&b.id()).unwrap().addr(),
        Some(("127.0.0.1".into(), b.port))
    );
}

#[test]
fn new_hello_to_an_older_peer_falls_back() {
    let (a, b) = routed(true, false);
    // b reads hello as before `port` and `caps` existed
    b.flow.hooks().refuse_hello_fields = vec!["port", "caps"];
    a.flow.link(&b.id()).unwrap().close();
    assert!(wait_until(5.0, || a.flow.link(&b.id()).unwrap().online()));
    assert!(a.flow.link(&b.id()).unwrap().caps().is_empty());
    // with only `caps` unknown, `port` still goes through
    b.flow.hooks().refuse_hello_fields = vec!["caps"];
    a.flow.link(&b.id()).unwrap().close();
    assert!(wait_until(5.0, || a.flow.link(&b.id()).unwrap().online()));
    assert!(a.flow.link(&b.id()).unwrap().caps().is_empty());
    b.flow.hooks().refuse_hello_fields = vec![];
    a.flow.link(&b.id()).unwrap().close();
    assert!(wait_until(5.0, || a.flow.link(&b.id()).unwrap().online()));
    assert!(a.flow.link(&b.id()).unwrap().caps().contains("transfer"));
}

#[test]
fn directed_announcements_only_to_paired_peers() {
    let (a, b) = routed(false, true); // discovery on, but no broadcast reaches anyone
    let c = Computer::new("equipo-c", Opts::default()).enable();
    assert_eq!(
        a.flow.discovery_destinations().unwrap(),
        vec![("127.0.0.1".to_string(), b.port)]
    );
    let link = b.flow.link(&a.id()).unwrap();
    link.set_hint(None);
    b.flow.clear_discovered();
    assert!(wait_until(3.0, || link.hint() == Some(("127.0.0.1".into(), a.port))));
    assert!(b.flow.discovered_ids().contains(&a.id()));
    sleep(0.8);
    assert!(c.flow.discovered_ids().is_empty()); // nothing for the unpaired computer
    assert_eq!(c.flow.discovery_destinations().unwrap(), vec![]);
}

#[test]
fn markup_in_names_is_dropped() {
    let (_a, b) = two_computers();
    let evil = "<p style=\"font-size:48px;color:red\">482 913</p>";
    b.flow.seen_for_tests(
        "announce",
        "0123456789abcdef",
        evil,
        1,
        "127.0.0.1:1".parse().unwrap(),
    );
    let d = b.call("flow_peers", json!({})).unwrap()["discovered"].clone();
    let names: Vec<&str> = d
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert!(!names.is_empty());
    assert!(names.iter().all(|n| !n.contains(['<', '>', '&'])));
}
