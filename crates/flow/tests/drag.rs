//! Files dragged across the edge between two Rust computers on loopback, with the fake
//! backend standing in for the desktop: the glow while files are dragged anywhere, the edge
//! catcher, and on the other computer the pickup that lets the drag go on
//! (docs/FLOW-DRAG-DROP.md).

mod common;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::*;
use glisa_flow::SendReason;
use glisa_flow::backends::PickupStep;
use glisa_flow::geometry::GlowEdge;
use glisa_flow::service::DRAG_WAIT;
use serde_json::json;

struct Setup {
    a: Computer,
    b: Computer,
    hub: Arc<SimHub>,
    src: PathBuf,
}

/// The mouse is on `c` and knows which channel `other` is on.
fn ready(c: &Computer, other: &Computer) -> bool {
    wait_until(5.0, || {
        c.flow
            .devices_status()
            .iter()
            .find(|d| d["online"] == true)
            .is_some_and(|d| !d["channels"][other.id()]["suggested"].is_null())
    })
}

fn setup() -> Setup {
    let (a, b) = paired_pair();
    let hub = shared_device(&a, &b);
    assert!(ready(&a, &b));
    let src = a.tmp.path().join("src");
    fs::create_dir_all(src.join("Proyecto/docs")).unwrap();
    fs::write(src.join("Proyecto/léeme.txt"), b"hola\n").unwrap();
    fs::write(src.join("Proyecto/docs/datos.bin"), vec![7u8; 300_000]).unwrap();
    fs::write(src.join("foto 1.jpg"), b"jpeg?").unwrap();
    Setup { a, b, hub, src }
}

const A_SCREEN: [i64; 4] = [0, 0, 1920, 1080];
const B_SCREEN: [i64; 4] = [0, 0, 2560, 1440];

impl Setup {
    fn dragged(&self) -> Vec<PathBuf> {
        vec![self.src.join("Proyecto"), self.src.join("foto 1.jpg")]
    }

    /// a's user holds a button and pushes the pointer against the right edge.
    fn hold_at_edge(&self) {
        *self.a.backend.buttons.lock().unwrap() = Some(true);
        self.a.backend.away();
        self.a.backend.hit("right", 0.5);
    }

    fn catcher_up(&self) -> bool {
        wait_until(3.0, || {
            self.a.backend.catcher() == Some(("right", A_SCREEN))
        })
    }

    /// Files dragged to the right edge cross with the mouse.
    fn drag_across(&self) {
        self.hold_at_edge();
        assert!(self.catcher_up(), "{:?}", self.a.backend.catcher_log());
        assert!(self.a.backend.drag_files(&self.dragged()));
        assert!(
            wait_until(5.0, || self.hub.moves() == vec![1]),
            "{:?} {:?}",
            self.hub.moves(),
            self.a.events()
        );
    }

    /// The transfer of the dragged files, as b sees it.
    fn received(&self) -> serde_json::Value {
        self.b.last_transfer()
    }

    fn on_b_clipboard(&self) -> bool {
        self.b.backend.rich_data().contains_key("text/uri-list")
    }
}

fn right_edge() -> Vec<GlowEdge> {
    vec![GlowEdge {
        side: "right",
        output: A_SCREEN,
        start: 0,
        len: 1080,
    }]
}

// ======================================================================= the glow

#[test]
fn edges_glow_while_files_are_dragged_anywhere() {
    let s = setup();
    let a = &s.a;
    assert!(a.backend.watching.load(Ordering::SeqCst)); // on by default
    // files picked up in the middle of the screen: the edge that leads to b glows
    assert!(a.backend.start_file_drag());
    assert!(wait_until(3.0, || a.backend.glow() == right_edge()));
    sleep(0.2);
    assert!(s.hub.moves().is_empty()); // nothing crosses for that
    assert!(a.backend.catcher_log().is_empty());
    // let go (dropped or cancelled): gone
    a.backend.end_file_drag();
    assert!(wait_until(3.0, || a.backend.glow().is_empty()));
    assert_eq!(a.backend.glow_log(), vec![right_edge(), vec![]]);
    // a stray "let go" changes nothing
    a.backend.end_file_drag();
    sleep(0.2);
    assert_eq!(a.backend.glow_log().len(), 2);
}

#[test]
fn the_glow_needs_what_the_catcher_needs() {
    let s = setup();
    let a = &s.a;
    // the option off: the watch stops, nothing is looked for
    a.call("flow_set_options", json!({"drag_files": false}))
        .unwrap();
    assert!(!a.backend.watching.load(Ordering::SeqCst));
    assert!(!a.backend.start_file_drag());
    a.call("flow_set_options", json!({"drag_files": true}))
        .unwrap();
    assert!(a.backend.watching.load(Ordering::SeqCst));
    a.call("flow_set_options", json!({"files": false})).unwrap();
    assert!(!a.backend.watching.load(Ordering::SeqCst));
    a.call("flow_set_options", json!({"files": true})).unwrap();
    // this desktop has no catcher: no glow either (it would promise a crossing)
    a.backend.catcher_supported.store(false, Ordering::SeqCst);
    assert!(!a.backend.start_file_drag());
    a.backend.catcher_supported.store(true, Ordering::SeqCst);
    // the mouse isn't here (it's on b): nothing could cross
    a.host.set_online(LIFT, false, 1);
    assert!(!a.backend.start_file_drag());
    a.host.set_online(LIFT, true, 0);
    assert!(a.backend.start_file_drag());
    assert!(wait_until(3.0, || !a.backend.glow().is_empty()));
    a.backend.end_file_drag();
    assert!(wait_until(3.0, || a.backend.glow().is_empty()));
    // nobody on that side any more
    a.flow.store.set_side(&s.b.id(), None).unwrap();
    assert!(!a.backend.start_file_drag());
    a.flow.store.set_side(&s.b.id(), Some("right")).unwrap();
    // the peer is offline
    s.b.flow.stop();
    assert!(wait_until(15.0, || a.peer_state(&s.b)["online"] == false));
    assert!(!a.backend.start_file_drag());
    sleep(0.2);
    assert_eq!(a.backend.glow_log().len(), 2);
}

#[test]
fn every_outer_edge_on_that_side_glows() {
    let s = setup();
    let a = &s.a;
    // two monitors, the second one taller; b above them
    let screens = vec![[0, 0, 1920, 1080], [1920, -360, 2560, 1440]];
    a.backend.report_screens(screens.clone());
    a.flow.store.set_side(&s.b.id(), Some("top")).unwrap();
    assert!(a.backend.start_file_drag());
    let want = vec![
        GlowEdge {
            side: "top",
            output: screens[0],
            start: 0,
            len: 1920,
        },
        GlowEdge {
            side: "top",
            output: screens[1],
            start: 0,
            len: 2560,
        },
    ];
    assert!(
        wait_until(3.0, || a.backend.glow() == want),
        "{:?}",
        a.backend.glow()
    );
    a.backend.end_file_drag();
    // b on the left: the first monitor's left edge, and the part of the taller one that
    // sticks out above it
    a.flow.store.set_side(&s.b.id(), Some("left")).unwrap();
    assert!(a.backend.start_file_drag());
    let want = vec![
        GlowEdge {
            side: "left",
            output: screens[0],
            start: 0,
            len: 1080,
        },
        GlowEdge {
            side: "left",
            output: screens[1],
            start: 0,
            len: 360,
        },
    ];
    assert!(
        wait_until(3.0, || a.backend.glow() == want),
        "{:?}",
        a.backend.glow()
    );
}

#[test]
fn the_glow_goes_by_itself() {
    let s = setup();
    let a = &s.a;
    a.flow.hooks().glow_timeout = Some(Duration::from_millis(300));
    assert!(a.backend.start_file_drag());
    assert!(wait_until(3.0, || !a.backend.glow().is_empty()));
    // the end of the drag went unseen: it goes anyway
    assert!(wait_until(3.0, || a.backend.glow().is_empty()));
    // turning the option off takes it away too
    a.flow.hooks().glow_timeout = None;
    assert!(a.backend.start_file_drag());
    assert!(wait_until(3.0, || !a.backend.glow().is_empty()));
    a.call("flow_set_options", json!({"drag_files": false}))
        .unwrap();
    assert!(wait_until(3.0, || a.backend.glow().is_empty()));
}

#[test]
fn with_the_glow_up_the_edge_still_crosses() {
    let s = setup();
    let a = &s.a;
    assert!(a.backend.start_file_drag());
    assert!(wait_until(3.0, || a.backend.glow() == right_edge()));
    // at the edge everything stays as it was: catcher, dwell, crossing
    s.drag_across();
    assert_eq!(a.backend.accepted(), vec![true]);
    // the mouse left: the glow and the catcher go
    assert!(wait_until(3.0, || a.backend.glow().is_empty()));
    assert!(wait_until(3.0, || a.backend.catcher().is_none()));
}

// ===================================================================== the catcher

#[test]
fn file_drag_crosses_and_arrives_as_copied_files() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    s.drag_across();
    assert_eq!(a.backend.accepted(), vec![true]); // the drop is taken as a copy
    assert!(wait_until(3.0, || a.backend.catcher().is_none()));
    // b can't let the drag go on (no pickup on its desktop): copied files, Ctrl+V
    assert!(wait_until(10.0, || s.received()["state"] == "done"));
    let t = s.received();
    assert_eq!(t["reason"], "drag");
    assert_eq!(t["placed"], "clipboard");
    assert_eq!(t["items"], 2);
    assert!(b.backend.pickup_log().is_empty());
    let uris = String::from_utf8(b.backend.rich_data()["text/uri-list"].clone()).unwrap();
    assert!(uris.contains("foto%201.jpg"), "{uris}");
    assert_eq!(
        glisa_flow::text::transfer_text(&t),
        "Received 2 items from equipo-a — paste them with Ctrl+V"
    );
    let sent = a
        .flow
        .transfers_status()
        .into_iter()
        .find(|t| t["direction"] == "send")
        .unwrap();
    assert_eq!(
        (sent["reason"].as_str(), sent["state"].as_str()),
        (Some("drag"), Some("done"))
    );
}

#[test]
fn plain_drag_does_not_cross_and_the_catcher_goes_with_the_button() {
    let s = setup();
    let a = &s.a;
    s.hold_at_edge();
    assert!(s.catcher_up());
    sleep(0.6); // nothing is dragged onto it: a window, text, a slider…
    assert!(s.hub.moves().is_empty());
    assert!(a.backend.accepted().is_empty());
    // the button is let go: the catcher goes, nothing crossed
    *a.backend.buttons.lock().unwrap() = Some(false);
    assert!(wait_until(3.0, || a.backend.catcher().is_none()));
    assert_eq!(a.backend.catcher_log().last(), Some(&None));
    assert!(!a.backend.drag_files(&s.dragged())); // nothing to drop onto any more
    sleep(0.3);
    assert!(s.hub.moves().is_empty());
    assert!(s.b.last_transfer()["state"].is_null());
    // as before: pushing the edge again without a button crosses
    a.backend.hit("right", 0.55);
    assert!(wait_until(5.0, || s.hub.moves() == vec![1]));
}

#[test]
fn leaving_the_edge_takes_the_catcher_away() {
    let s = setup();
    s.hold_at_edge();
    assert!(s.catcher_up());
    s.a.backend.away();
    assert!(wait_until(3.0, || s.a.backend.catcher().is_none()));
    assert!(!s.a.backend.drag_files(&s.dragged()));
    sleep(0.3);
    assert!(s.hub.moves().is_empty());
}

#[test]
fn released_before_the_switch_nothing_crosses() {
    let s = setup();
    let a = &s.a;
    a.backend.drop_before_accept.store(true, Ordering::SeqCst);
    s.hold_at_edge();
    assert!(s.catcher_up());
    assert!(a.backend.drag_files(&s.dragged()));
    assert!(wait_until(5.0, || !a.backend.accepted().is_empty()));
    assert!(wait_until(3.0, || a.backend.catcher().is_none()));
    sleep(0.4);
    assert!(s.hub.moves().is_empty()); // the drop was refused: the mouse stays
    assert!(
        a.flow
            .transfers_status()
            .iter()
            .all(|t| t["direction"] != "send")
    );
}

#[test]
fn no_peer_no_catcher() {
    let s = setup();
    let a = &s.a;
    *a.backend.buttons.lock().unwrap() = Some(true);
    // an edge with nobody behind it
    a.backend.hit("left", 0.5);
    sleep(0.3);
    assert!(a.backend.catcher_log().is_empty());
    // this desktop has no catcher
    a.backend.catcher_supported.store(false, Ordering::SeqCst);
    a.backend.away();
    a.backend.hit("right", 0.5);
    sleep(0.3);
    assert!(a.backend.catcher_log().is_empty());
    a.backend.catcher_supported.store(true, Ordering::SeqCst);
    // files aren't sent from here
    a.call("flow_set_options", json!({"files": false})).unwrap();
    a.backend.away();
    a.backend.hit("right", 0.5);
    sleep(0.3);
    assert!(a.backend.catcher_log().is_empty());
    a.call("flow_set_options", json!({"files": true})).unwrap();
    // the peer is offline
    s.b.flow.stop();
    assert!(wait_until(15.0, || a.peer_state(&s.b)["online"] == false));
    a.backend.away();
    a.backend.hit("right", 0.5);
    sleep(0.3);
    assert!(a.backend.catcher_log().is_empty());
    assert!(s.hub.moves().is_empty());
}

// ====================================================== on the other computer: pickup

#[test]
fn the_drag_goes_on_where_the_mouse_arrives() {
    let s = setup();
    let b = &s.b;
    b.backend.pickup_supported.store(true, Ordering::SeqCst);
    s.drag_across();
    // b caught the notice and the switch: the pickup waits on the output where the
    // pointer enters, then the mouse arrives and arms it
    let expect = PickupStep::Expect {
        output: B_SCREEN,
        count: 2,
    };
    assert!(
        wait_until(5.0, || {
            let log = b.backend.pickup_log();
            log.first() == Some(&expect) && log.contains(&PickupStep::Arrived)
        }),
        "{:?}",
        b.backend.pickup_log()
    );
    // the files arrive and are handed to the pickup, not to the clipboard
    assert!(wait_until(10.0, || {
        b.backend
            .pickup_log()
            .iter()
            .any(|x| matches!(x, PickupStep::Ready(_)))
    }));
    sleep(0.2);
    assert_eq!(s.received()["state"], "receiving");
    assert_eq!(s.received()["percent"], 100);
    assert!(!s.on_b_clipboard());
    // the press of the arriving mouse: the drag goes on; dropped into a folder
    assert!(b.backend.pickup_press());
    let got = b.backend.pickup_drop(true);
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|p| p.starts_with(&b.flow.staging.root)));
    let names: HashSet<String> = got
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        HashSet::from(["Proyecto".to_string(), "foto 1.jpg".to_string()])
    );
    assert_eq!(
        fs::read(got.iter().find(|p| p.ends_with("foto 1.jpg")).unwrap()).unwrap(),
        b"jpeg?"
    );
    // done: dropped, nothing on the clipboard, nothing to paste
    assert!(wait_until(3.0, || s.received()["state"] == "done"));
    assert_eq!(s.received()["placed"], "drop");
    assert!(!s.on_b_clipboard());
    assert_eq!(
        glisa_flow::text::transfer_text(&s.received()),
        "Received 2 items from equipo-a"
    );
    assert!(!b.backend.pickup_log().contains(&PickupStep::Cancel));
}

#[test]
fn no_press_after_the_arrival_means_ctrl_v() {
    let s = setup();
    let b = &s.b;
    b.backend.pickup_supported.store(true, Ordering::SeqCst);
    b.flow.hooks().pickup_press_wait = Some(Duration::from_millis(400));
    s.drag_across();
    // the user had let go: no press comes, the pickup goes (it would swallow clicks)
    assert!(
        wait_until(10.0, || b
            .backend
            .pickup_log()
            .contains(&PickupStep::Cancel)),
        "{:?}",
        b.backend.pickup_log()
    );
    assert!(!b.backend.pickup_press());
    // the files arrive as copied files: on the clipboard, with the usual notice
    assert!(wait_until(10.0, || s.received()["state"] == "done"));
    assert_eq!(s.received()["placed"], "clipboard");
    assert!(s.on_b_clipboard());
}

#[test]
fn a_cancelled_drag_leaves_them_on_the_clipboard() {
    let s = setup();
    let b = &s.b;
    b.backend.pickup_supported.store(true, Ordering::SeqCst);
    s.drag_across();
    assert!(wait_until(5.0, || b
        .backend
        .pickup_log()
        .contains(&PickupStep::Arrived)));
    assert!(b.backend.pickup_press());
    // Escape, or a drop somewhere that refuses files
    assert!(b.backend.pickup_drop(false).is_empty());
    assert!(wait_until(10.0, || s.received()["state"] == "done"));
    assert_eq!(s.received()["placed"], "clipboard");
    assert!(s.on_b_clipboard());
}

#[test]
fn older_peer_gets_the_files_without_a_notice() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    b.backend.pickup_supported.store(true, Ordering::SeqCst);
    // b as a Glisa from before `drag`: it takes transfers, not notices
    let link = a.flow.link(&b.id()).unwrap();
    assert!(link.caps().contains("drag"));
    link.set_caps(HashSet::from(["transfer".to_string()]));
    s.drag_across();
    assert!(wait_until(10.0, || b.last_transfer()["state"] == "done"));
    assert!(b.backend.pickup_log().is_empty()); // no notice: no pickup
    assert_eq!(b.last_transfer()["reason"], "clipboard"); // as copied files: Ctrl+V
    assert!(s.on_b_clipboard());
}

#[test]
fn notice_round_trip_and_matching() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    let link = a.flow.link(&b.id()).unwrap();
    // the notice itself: acknowledged
    let r = link
        .request(
            "drag",
            Duration::from_secs(2),
            json!({"names": ["Proyecto"], "count": 1, "bytes": 10}),
        )
        .unwrap();
    assert_eq!(r["type"], "ack");
    // a transfer that isn't what was announced is ordinary copied files
    let other = s.src.join("otra cosa.txt");
    fs::write(&other, b"x").unwrap();
    a.flow
        .send_paths(&b.id(), vec![other.clone()], SendReason::Clipboard)
        .unwrap();
    assert!(wait_until(10.0, || b.last_transfer()["state"] == "done"));
    assert_eq!(b.last_transfer()["reason"], "clipboard");
    // the announced one is the dragged files (no switch came: no pickup, Ctrl+V)
    a.flow
        .send_paths(&b.id(), vec![s.src.join("Proyecto")], SendReason::Drag)
        .unwrap();
    assert!(wait_until(10.0, || b
        .flow
        .transfers_status()
        .iter()
        .filter(|t| t["state"] == "done")
        .count()
        == 2));
    assert_eq!(b.last_transfer()["reason"], "drag");
    assert_eq!(b.last_transfer()["placed"], "clipboard");
    // only once per notice
    a.flow
        .send_paths(&b.id(), vec![s.src.join("Proyecto")], SendReason::Drag)
        .unwrap();
    assert!(wait_until(10.0, || b
        .flow
        .transfers_status()
        .iter()
        .filter(|t| t["state"] == "done")
        .count()
        == 3));
    assert_eq!(b.last_transfer()["reason"], "clipboard");
    assert!(DRAG_WAIT >= Duration::from_secs(60));
}

#[test]
fn the_option_turns_it_off() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    // on by default, shown by flow_status, set with flow_set_options
    assert_eq!(
        a.call("flow_status", json!({})).unwrap()["options"]["drag_files"],
        true
    );
    let r = a
        .call("flow_set_options", json!({"drag_files": false}))
        .unwrap();
    assert_eq!(r["drag_files"], false);
    assert_eq!(
        a.call("flow_status", json!({})).unwrap()["options"]["drag_files"],
        false
    );
    assert!(
        a.call("flow_set_options", json!({"drag_files": "no"}))
            .is_err()
    );
    // off here: no catcher, a drag to the edge stays a plain drag
    s.hold_at_edge();
    sleep(0.4);
    assert!(a.backend.catcher_log().is_empty());
    assert!(s.hub.moves().is_empty());
    a.call("flow_set_options", json!({"drag_files": true}))
        .unwrap();
    // off on b: peers stop hearing `drag` (their channel says hello again) and b never
    // picks a drag up; dragged files still arrive there as copied files
    b.backend.pickup_supported.store(true, Ordering::SeqCst);
    assert!(a.flow.link(&b.id()).unwrap().caps().contains("drag"));
    b.call("flow_set_options", json!({"drag_files": false}))
        .unwrap();
    assert!(!b.flow.caps().contains(&"drag"));
    assert!(wait_until(8.0, || {
        let l = a.flow.link(&b.id()).unwrap();
        // the next request reconnects
        let _ = l.request("ping", Duration::from_secs(1), json!({}));
        l.online() && !l.caps().contains("drag") && l.caps().contains("transfer")
    }));
    let link = a.flow.link(&b.id()).unwrap();
    link.request(
        "drag",
        Duration::from_secs(2),
        json!({"names": ["Proyecto"], "count": 1, "bytes": 1}),
    )
    .unwrap(); // a stale peer: acknowledged and ignored
    *a.backend.buttons.lock().unwrap() = Some(true);
    a.backend.away();
    a.backend.hit("right", 0.5);
    assert!(s.catcher_up());
    assert!(a.backend.drag_files(&s.dragged()));
    assert!(wait_until(5.0, || s.hub.moves() == vec![1]));
    assert!(wait_until(10.0, || b.last_transfer()["state"] == "done"));
    assert_eq!(b.last_transfer()["reason"], "clipboard");
    assert!(b.backend.pickup_log().is_empty());
    // and back on: `drag` is announced again
    b.call("flow_set_options", json!({"drag_files": true}))
        .unwrap();
    assert!(b.flow.caps().contains(&"drag"));
}

#[test]
fn no_pickup_while_our_mouse_is_here() {
    let s = setup();
    let (a, b) = (&s.a, &s.b);
    b.backend.pickup_supported.store(true, Ordering::SeqCst);
    let link = a.flow.link(&b.id()).unwrap();
    let notice_and_switch = || {
        link.request(
            "drag",
            Duration::from_secs(2),
            json!({"names": ["Proyecto"], "count": 1, "bytes": 1}),
        )
        .unwrap();
        let r = link
            .request(
                "switch",
                Duration::from_secs(2),
                json!({"side": "right", "t": 0.5, "devices": [{"name": "LIFT", "kind": "mouse"}]}),
            )
            .unwrap();
        assert_eq!(r["type"], "switch_ok");
    };
    // b's user is using a Logitech mouse there: no drag can be coming with one
    b.host.set_device(glisa_common::FlowDevice {
        id: "c041-mx".into(),
        key: "C041:MX".into(),
        name: "MX".into(),
        kind: glisa_common::DeviceKind::Mouse,
        online: true,
        hosts_count: 3,
        current_host: 1,
        host_entries: Vec::new(),
    });
    notice_and_switch();
    sleep(0.3);
    assert!(b.backend.pickup_log().is_empty());
    // it went elsewhere: now a drag can come
    b.host.set_online("c041-mx", false, 0);
    sleep(1.0); // past the cooldown of that switch
    notice_and_switch();
    assert!(wait_until(3.0, || matches!(
        b.backend.pickup_log().first(),
        Some(PickupStep::Expect { count: 1, .. })
    )));
}
