//! `glisa-ctl flow …`: Flow through the service's `flow_*` API.

use std::io::{self, Write};
use std::time::Duration;

use glisa_i18n::{tr, trc};
use glisa_service::{Client, ClientError};
use serde_json::{Map, Value, json};

use crate::args::{Parsed, Pos, Spec, flag, opt, pos};
use crate::text::{human_size, is_yes, size_text, transfer_text, transfer_what};
use crate::{Io, pick};

pub const SIDES: &[&str] = &["left", "right", "top", "bottom"];

/// "left" -> the word for it in the current language.
pub fn side_name(side: Option<&str>) -> String {
    match side.unwrap_or("") {
        "left" => trc!("screen side", "left"),
        "right" => trc!("screen side", "right"),
        "top" => trc!("screen side", "top"),
        "bottom" => trc!("screen side", "bottom"),
        _ => tr!("not placed"),
    }
}

/// Accepted values for `flow layout … SIDE`: English and the current language.
pub fn side_choices() -> Vec<(String, &'static str)> {
    let mut out: Vec<(String, &'static str)> = SIDES.iter().map(|s| (s.to_string(), *s)).collect();
    out.push(("none".into(), "none"));
    for s in SIDES {
        let local = side_name(Some(s));
        if !out.iter().any(|(k, _)| *k == local) {
            out.push((local, s));
        }
    }
    let none = trc!("screen side", "none");
    if !out.iter().any(|(k, _)| *k == none) {
        out.push((none, "none"));
    }
    out
}

pub fn connection(online: bool) -> String {
    if online {
        trc!("connection", "online")
    } else {
        trc!("connection", "offline")
    }
}

fn yes_no(v: &Value) -> String {
    if crate::truthy(v) {
        trc!("option value", "yes")
    } else {
        trc!("option value", "no")
    }
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn b(v: &Value, key: &str) -> bool {
    v.get(key).is_some_and(crate::truthy)
}

fn list<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn remote(msg: String) -> ClientError {
    ClientError::Remote(msg)
}

/// The peer called `ident` (id, exact name, or the only name containing it).
pub fn find_peer(items: &[Value], ident: &str) -> Result<Value, ClientError> {
    if let Some(p) = items
        .iter()
        .find(|p| s(p, "id") == ident || s(p, "name") == ident)
    {
        return Ok(p.clone());
    }
    let low = ident.to_lowercase();
    let hits: Vec<&Value> = items
        .iter()
        .filter(|p| s(p, "name").to_lowercase().contains(&low))
        .collect();
    match hits.len() {
        1 => Ok(hits[0].clone()),
        0 => Err(remote(tr!(
            "Can't find the computer “{name}”",
            name = ident
        ))),
        _ => Err(remote(tr!(
            "“{name}” matches more than one computer",
            name = ident
        ))),
    }
}

fn address_source(p: &Value) -> String {
    match s(p, "address_source") {
        "manual" => tr!("set manually"),
        "learned" | "discovered" => tr!("found automatically"),
        _ => String::new(),
    }
}

fn print_peer_address(out: &mut dyn Write, p: &Value) -> io::Result<()> {
    let name = s(p, "name");
    if !s(p, "address").is_empty() {
        writeln!(
            out,
            "{}",
            tr!(
                "{name}: {address} ({source})",
                name = name,
                address = s(p, "address"),
                source = address_source(p)
            )
        )?;
    } else {
        writeln!(out, "{}", tr!("{name}: address not known yet", name = name))?;
    }
    let manual = match s(p, "manual_address") {
        "" => trc!("address", "none (automatic)"),
        m => m.to_string(),
    };
    writeln!(
        out,
        "  {}",
        tr!("manual address: {address}", address = manual)
    )?;
    let learned = s(p, "learned_address");
    if !learned.is_empty() && learned != s(p, "address") {
        writeln!(
            out,
            "  {}",
            tr!(
                "last address where it was reached: {address}",
                address = learned
            )
        )?;
    }
    if !b(p, "online") {
        let err = s(p, "last_error");
        let extra = if err.is_empty() {
            String::new()
        } else {
            format!(": {err}")
        };
        writeln!(out, "  {}{extra}", connection(false))?;
        writeln!(
            out,
            "  {}",
            tr!(
                "If it's on a different network, set its address: glisa-ctl flow address {name} HOST[:PORT]",
                name = name
            )
        )?;
    }
    Ok(())
}

fn print_transfers(out: &mut dyn Write, transfers: &[Value]) -> io::Result<()> {
    let active: Vec<&Value> = transfers.iter().filter(|t| b(t, "active")).collect();
    let shown: Vec<&Value> = if active.is_empty() {
        transfers
            .last()
            .into_iter()
            .filter(|t| t.get("ago").and_then(Value::as_f64).unwrap_or(0.0) < 600.0)
            .collect()
    } else {
        active
    };
    for t in shown {
        let mut line = transfer_text(t);
        let bytes = t.get("bytes").and_then(Value::as_i64).unwrap_or(0);
        if b(t, "active") && bytes != 0 {
            let done = t.get("done").and_then(Value::as_i64).unwrap_or(0);
            line.push_str(&format!(
                "  ({})",
                tr!(
                    "{done} of {total}",
                    done = human_size(done),
                    total = human_size(bytes)
                )
            ));
        }
        writeln!(out, "  {line}")?;
        if b(t, "active") {
            writeln!(out, "    {}", tr!("to cancel it: glisa-ctl flow cancel"))?;
        } else if s(t, "direction") == "receive"
            && s(t, "state") == "done"
            && !s(t, "folder").is_empty()
        {
            writeln!(out, "    {}", tr!("folder: {path}", path = s(t, "folder")))?;
        }
    }
    Ok(())
}

fn print_status(out: &mut dyn Write, st: &Value) -> io::Result<()> {
    let state = if b(st, "running") {
        trc!("flow state", "on")
    } else {
        trc!("flow state", "off")
    };
    let fp = match s(st, "fingerprint") {
        "" => "-",
        f => f,
    };
    writeln!(
        out,
        "{}",
        tr!(
            "Flow: {state}  ·  this computer: {name}  ·  fingerprint {fingerprint}",
            state = state,
            name = s(st, "name"),
            fingerprint = fp
        )
    )?;
    if !b(st, "available") || matches!(s(st, "backend_state"), "error" | "unavailable") {
        let msg = match s(st, "backend_message") {
            "" => tr!("edge detection not available"),
            m => m.to_string(),
        };
        writeln!(out, "  ! {msg}")?;
    }
    if !s(st, "error").is_empty() {
        writeln!(out, "  ! {}", s(st, "error"))?;
    }
    let ports = st.get("ports").cloned().unwrap_or(json!({}));
    let port = |k: &str| ports.get(k).map(Value::to_string).unwrap_or_default();
    writeln!(
        out,
        "  {}",
        tr!(
            "ports: UDP {udp} (discovery), TCP {tcp} (computers), TCP {pairing} (only while pairing)",
            udp = port("udp"),
            tcp = port("tcp"),
            pairing = port("pairing")
        )
    )?;
    if st.get("firewall").is_some_and(|f| b(f, "ufw")) {
        let cmd = format!(
            "sudo ufw allow {}/udp && sudo ufw allow {}:{}/tcp",
            port("udp"),
            port("tcp"),
            port("pairing")
        );
        writeln!(
            out,
            "  {}",
            tr!("ufw firewall is active: {command}", command = cmd)
        )?;
    }
    let o = st.get("options").cloned().unwrap_or(json!({}));
    let num = |k: &str| o.get(k).map(Value::to_string).unwrap_or_default();
    writeln!(
        out,
        "  {}",
        tr!(
            "options: Ctrl={ctrl}  clipboard={clipboard}  keyboard={keyboard}  cooldown={cooldown} ms  corners={corner} px",
            ctrl = yes_no(o.get("require_ctrl").unwrap_or(&Value::Null)),
            clipboard = yes_no(o.get("clipboard").unwrap_or(&Value::Null)),
            keyboard = yes_no(o.get("keyboard").unwrap_or(&Value::Null)),
            cooldown = num("cooldown_ms"),
            corner = num("corner_px")
        )
    )?;
    if let Some(files) = o.get("files") {
        let mb = |k: &str| o.get(k).and_then(Value::as_i64).unwrap_or(0);
        writeln!(
            out,
            "  {}",
            tr!(
                "copied files and images: {state}, up to {limit} per copy (images up to {image})",
                state = yes_no(files),
                limit = size_text(mb("max_transfer_mb")),
                image = size_text(mb("max_image_mb"))
            )
        )?;
    }
    if let Some(drag) = o.get("drag_files") {
        writeln!(
            out,
            "  {}",
            tr!(
                "files dragged across the edge: {state}",
                state = yes_no(drag)
            )
        )?;
    }
    print_transfers(out, list(st, "transfers"))?;
    let peers = list(st, "peers");
    writeln!(
        out,
        "{}",
        if peers.is_empty() {
            tr!("No paired computers")
        } else {
            tr!("Paired computers:")
        }
    )?;
    for p in peers {
        let online = match s(p, "state") {
            "online" => trc!("connection", "online"),
            "identity" => tr!("IDENTITY CHANGED"),
            _ => trc!("connection", "offline"),
        };
        writeln!(
            out,
            "  {:<24} {:<18} {:<12} {}",
            s(p, "name"),
            online,
            side_name(p.get("side").and_then(Value::as_str)),
            tr!(
                "fingerprint {fingerprint}",
                fingerprint = s(p, "fingerprint")
            )
        )?;
    }
    for d in list(st, "devices") {
        let current = d
            .get("hosts")
            .and_then(|h| h.get("current"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        writeln!(
            out,
            "  {}",
            tr!(
                "{device} (current channel {channel}):",
                device = s(d, "name"),
                channel = current + 1
            )
        )?;
        if let Some(Value::Object(chs)) = d.get("channels") {
            for (pid, ch) in chs {
                let name = peers
                    .iter()
                    .find(|p| s(p, "id") == pid)
                    .map_or(pid.as_str(), |p| s(p, "name"));
                let chosen = ch.get("channel").and_then(Value::as_i64);
                let val = chosen.or_else(|| ch.get("suggested").and_then(Value::as_i64));
                let how = if chosen.is_some() {
                    trc!("channel", "chosen")
                } else if val.is_some() {
                    trc!("channel", "suggested")
                } else {
                    trc!("channel", "not chosen")
                };
                let shown = val.map_or("-".to_string(), |v| (v + 1).to_string());
                writeln!(
                    out,
                    "      {name:<20} {}",
                    tr!("channel {channel} ({how})", channel = shown, how = how)
                )?;
            }
        }
    }
    Ok(())
}

fn on_off(p: &Parsed, key: &str) -> Option<Value> {
    p.get(key).map(|v| json!(v == "on"))
}

fn pair(c: &mut Client, target: Option<&str>, io: &mut Io) -> Result<u8, ClientError> {
    if let Some(target) = target {
        let found = c.call("flow_discover", json!({"wait": 1.0}))?;
        let peer = find_peer(list(&found, "discovered"), target);
        match peer {
            Ok(p) => {
                c.call("flow_pair_request", json!({"peer": s(&p, "id")}))?;
            }
            Err(e) => {
                if !target.contains(':') && !target.contains('.') {
                    return Err(e);
                }
                c.call("flow_pair_request", json!({"address": target}))?;
            }
        }
        let _ = writeln!(io.out, "{}", tr!("Connecting…"));
    } else {
        let st = c.call("flow_pair_start", Value::Null)?;
        let secs = st
            .get("expires_in")
            .map(Value::to_string)
            .unwrap_or_default();
        let _ = writeln!(
            io.out,
            "{}",
            tr!(
                "Pairing is open for {seconds} s. On the other computer, choose this one in Glisa → Flow → Pair a computer (or run there: glisa-ctl flow pair NAME)",
                seconds = secs
            )
        );
    }
    crate::interrupt::install();
    loop {
        if crate::interrupt::triggered() {
            let _ = c.call("flow_pair_cancel", Value::Null);
            let _ = writeln!(io.out, "\n{}", tr!("Pairing cancelled"));
            return Ok(1);
        }
        let p = c.call("flow_status", Value::Null)?;
        let p = p.get("pairing").cloned().unwrap_or(Value::Null);
        if p.is_null() {
            let _ = writeln!(io.out, "{}", tr!("Pairing cancelled"));
            return Ok(1);
        }
        let peer_name = p
            .get("peer")
            .map(|x| s(x, "name").to_string())
            .unwrap_or_default();
        match s(&p, "state") {
            "confirm" if p.get("local").is_none_or(Value::is_null) => {
                let code = s(&p, "sas");
                let (a, b) = code.split_at(code.len().min(3));
                let _ = writeln!(
                    io.out,
                    "\n{}\n\n    {a} {b}\n",
                    tr!("Check that {name} shows the same number:", name = peer_name)
                );
                let answer = (io.ask)(&tr!("Do they match? [y/N] "));
                if crate::interrupt::triggered() {
                    continue; // cancelled at the top of the loop
                }
                let accept = is_yes(&answer.unwrap_or_default());
                c.call("flow_pair_confirm", json!({"accept": accept}))?;
                let _ = writeln!(io.out, "{}", tr!("Waiting for the other computer…"));
            }
            "done" => {
                let _ = writeln!(io.out, "{}", tr!("Paired with {name}", name = peer_name));
                return Ok(0);
            }
            "failed" | "expired" => {
                let _ = writeln!(
                    io.out,
                    "{}",
                    tr!("Not paired: {reason}", reason = s(&p, "message"))
                );
                return Ok(1);
            }
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// The `flow` subcommands.
pub fn specs() -> Vec<Spec> {
    let peer = || pos("peer", tr!("COMPUTER"));
    let mut v = vec![
        Spec {
            name: "status",
            help: tr!("state of Flow"),
            positionals: vec![],
            options: vec![flag("json", "--json", None, tr!("machine-readable output"))],
        },
        simple("on", tr!("turn Flow on")),
        simple("off", tr!("turn Flow off")),
        simple(
            "peers",
            tr!("paired computers and those seen on the network"),
        ),
        simple("discover", tr!("look for computers on the network now")),
        Spec {
            name: "pair",
            help: tr!("without an argument: open pairing here; with COMPUTER: pair with it"),
            positionals: vec![
                pos("target", tr!("COMPUTER"))
                    .optional()
                    .help(tr!("name, id or IP:PORT")),
            ],
            options: vec![],
        },
        Spec {
            name: "unpair",
            help: tr!("forget a paired computer"),
            positionals: vec![peer()],
            options: vec![],
        },
        Spec {
            name: "layout",
            help: tr!("on which side of this screen a computer is"),
            positionals: vec![
                peer(),
                pos("side", tr!("SIDE"))
                    .choices(side_choices().into_iter().map(|(k, _)| k).collect())
                    .help(tr!("left, right, top, bottom or none")),
            ],
            options: vec![],
        },
        Spec {
            name: "address",
            help: tr!("show or set where a paired computer is (for another network)"),
            positionals: vec![
                peer(),
                pos("address", tr!("HOST[:PORT]")).optional().help(tr!(
                    "IP address or name, optionally with the port; auto to find it automatically"
                )),
            ],
            options: vec![],
        },
    ];
    let onoff = || vec!["on".to_string(), "off".to_string()];
    v.push(Spec {
        name: "options",
        help: tr!("change the Flow options"),
        positionals: vec![],
        options: vec![
            opt(
                "ctrl",
                "--ctrl",
                None,
                "on|off".into(),
                tr!("switch only while Ctrl is held"),
            )
            .choices(onoff()),
            opt(
                "clipboard",
                "--clipboard",
                None,
                "on|off".into(),
                tr!("bring the clipboard along"),
            )
            .choices(onoff()),
            opt(
                "files",
                "--files",
                None,
                "on|off".into(),
                tr!("bring copied files and images along"),
            )
            .choices(onoff()),
            opt(
                "drag",
                "--drag",
                None,
                "on|off".into(),
                tr!("drag files across the edge to the other computer"),
            )
            .choices(onoff()),
            opt(
                "keyboard",
                "--keyboard",
                None,
                "on|off".into(),
                tr!("keyboards follow the mouse"),
            )
            .choices(onoff()),
            opt(
                "cooldown",
                "--cooldown",
                None,
                "MS".into(),
                tr!("pause after switching"),
            ),
            opt(
                "corner",
                "--corner",
                None,
                "PX".into(),
                tr!("dead zone at the ends of each edge"),
            ),
            opt(
                "max_size",
                "--max-size",
                None,
                "MB".into(),
                tr!("largest copy of files sent or received (MB)"),
            ),
            opt(
                "image_max",
                "--image-max",
                None,
                "MB".into(),
                tr!("largest copied image sent or received (MB)"),
            ),
        ],
    });
    v.push(simple(
        "cancel",
        tr!("cancel the copied files or image being sent or received"),
    ));
    v.push(Spec {
        name: "channel",
        help: tr!("the Easy-Switch channel a device uses for a computer"),
        positionals: vec![
            pos("device", tr!("DEVICE")),
            peer(),
            pos("channel", tr!("CHANNEL")).help(tr!("1..N or auto")),
        ],
        options: vec![],
    });
    v.push(simple(
        "reset-identity",
        tr!("new identity for this computer (unpairs all)"),
    ));
    v.push(Spec {
        name: "test-edge",
        help: tr!("demo mode only"),
        positionals: vec![
            pos("side", "SIDE".into()).choices(SIDES.iter().map(|s| s.to_string()).collect()),
            Pos {
                key: "t",
                metavar: "T".into(),
                required: false,
                choices: None,
                help: None,
            },
        ],
        options: vec![],
    });
    v
}

fn simple(name: &'static str, help: String) -> Spec {
    Spec {
        name,
        help,
        positionals: vec![],
        options: vec![],
    }
}

fn print_json(out: &mut dyn Write, v: &Value) {
    let _ = writeln!(
        out,
        "{}",
        serde_json::to_string_pretty(v).unwrap_or_default()
    );
}

/// Runs `flow <cmd>`.
pub fn command(c: &mut Client, cmd: &str, a: &Parsed, io: &mut Io) -> Result<u8, ClientError> {
    match cmd {
        "status" => {
            let st = c.call("flow_status", Value::Null)?;
            if a.flag("json") {
                print_json(io.out, &st);
            } else {
                let _ = print_status(io.out, &st);
            }
            return Ok(0);
        }
        "on" | "off" => {
            let st = c.call("flow_enable", json!({"enabled": cmd == "on"}))?;
            let msg = if b(&st, "running") {
                tr!("Flow turned on")
            } else {
                tr!("Flow turned off")
            };
            let _ = writeln!(io.out, "{msg}");
            return Ok(0);
        }
        "peers" | "discover" => {
            let res = if cmd == "discover" {
                c.call("flow_discover", json!({"wait": 1.5}))?
            } else {
                c.call("flow_peers", Value::Null)?
            };
            let peers = list(&res, "peers");
            for p in peers {
                let addr = match s(p, "address") {
                    "" => "-",
                    a => a,
                };
                let _ = writeln!(
                    io.out,
                    "{:<11} {:<24} {:<14} {:<22} {}",
                    trc!("computer list", "paired"),
                    s(p, "name"),
                    connection(b(p, "online")),
                    addr,
                    s(p, "id")
                );
            }
            let discovered = list(&res, "discovered");
            for d in discovered {
                if b(d, "paired") {
                    continue;
                }
                let note = if b(d, "compatible") {
                    String::new()
                } else {
                    format!("  {}", tr!("(different version of Flow)"))
                };
                let _ = writeln!(
                    io.out,
                    "{:<11} {:<24} {:<22} {}{note}",
                    trc!("computer list", "on network"),
                    s(d, "name"),
                    s(d, "address"),
                    s(d, "id")
                );
            }
            if peers.is_empty() && discovered.is_empty() {
                let _ = writeln!(io.out, "{}", tr!("No computers found"));
            }
            return Ok(0);
        }
        "pair" => return pair(c, a.get("target"), io),
        "reset-identity" => {
            let st = c.call("flow_reset_identity", Value::Null)?;
            let _ = writeln!(
                io.out,
                "{}",
                tr!(
                    "New identity: fingerprint {fingerprint}. All computers have been unpaired",
                    fingerprint = s(&st, "fingerprint")
                )
            );
            return Ok(0);
        }
        "test-edge" => {
            let t: f64 = a.get("t").and_then(|t| t.parse().ok()).unwrap_or(0.5);
            c.call(
                "flow_simulate",
                json!({"op": "edge", "side": a.get("side"), "t": t}),
            )?;
            return Ok(0);
        }
        "cancel" => {
            let st = c.call("flow_status", Value::Null)?;
            let active: Vec<Value> = list(&st, "transfers")
                .iter()
                .filter(|t| b(t, "active"))
                .cloned()
                .collect();
            if active.is_empty() {
                let _ = writeln!(
                    io.out,
                    "{}",
                    tr!("No copied files or images are being sent or received")
                );
                return Ok(1);
            }
            for t in active {
                c.call("flow_transfer_cancel", json!({"transfer": t.get("id")}))?;
                let _ = writeln!(
                    io.out,
                    "{}",
                    tr!("Cancelled: {what}", what = transfer_what(&t))
                );
            }
            return Ok(0);
        }
        "options" => {
            let mut opts = Map::new();
            for (key, flag) in [
                ("require_ctrl", "ctrl"),
                ("clipboard", "clipboard"),
                ("files", "files"),
                ("drag_files", "drag"),
                ("keyboard", "keyboard"),
            ] {
                if let Some(v) = on_off(a, flag) {
                    opts.insert(key.into(), v);
                }
            }
            for (key, arg) in [
                ("cooldown_ms", "cooldown"),
                ("corner_px", "corner"),
                ("max_transfer_mb", "max_size"),
                ("max_image_mb", "image_max"),
            ] {
                if let Some(v) = a.get(arg) {
                    let n: i64 = v.trim().parse().map_err(|_| {
                        remote(tr!("{option} must be a whole number", option = arg))
                    })?;
                    opts.insert(key.into(), json!(n));
                }
            }
            let r = c.call("flow_set_options", Value::Object(opts))?;
            print_json(io.out, &r);
            return Ok(0);
        }
        _ => {}
    }
    let peers = c.call("flow_peers", Value::Null)?;
    let peer = find_peer(list(&peers, "peers"), a.get("peer").unwrap_or(""))?;
    let (pid, pname) = (s(&peer, "id").to_string(), s(&peer, "name").to_string());
    match cmd {
        "unpair" => {
            c.call("flow_remove_peer", json!({"peer": pid}))?;
            let _ = writeln!(io.out, "{}", tr!("{name} unpaired", name = pname));
        }
        "layout" => {
            let given = a.get("side").unwrap_or("none");
            let side = side_choices()
                .into_iter()
                .find(|(k, _)| k == given)
                .map_or(given.to_string(), |(_, v)| v.to_string());
            let value = if side == "none" {
                Value::Null
            } else {
                json!(side)
            };
            c.call("flow_set_layout", json!({"peer": pid, "side": value}))?;
            let _ = writeln!(io.out, "{pname}: {}", side_name(Some(&side)));
        }
        "address" => {
            let Some(address) = a.get("address") else {
                let _ = print_peer_address(io.out, &peer);
                return Ok(0);
            };
            let p = c.call(
                "flow_set_peer_address",
                json!({"peer": pid, "address": address}),
            )?;
            let manual = s(&p, "manual_address");
            let msg = if manual.is_empty() {
                tr!(
                    "{name}: its address will be found automatically",
                    name = s(&p, "name")
                )
            } else {
                tr!(
                    "{name}: address set to {address}",
                    name = s(&p, "name"),
                    address = manual
                )
            };
            let _ = writeln!(io.out, "{msg}");
        }
        "channel" => {
            let given = a.get("channel").unwrap_or("auto");
            let channel = if given == "auto" {
                Value::Null
            } else {
                let n: i64 = given
                    .trim()
                    .parse()
                    .map_err(|_| remote(tr!("Invalid channel")))?;
                json!(n - 1)
            };
            let device = pick(c, a.get("device"))?;
            c.call(
                "flow_set_channel",
                json!({"device": device, "peer": pid, "channel": channel}),
            )?;
            let _ = writeln!(
                io.out,
                "{}",
                tr!("{name}: channel {channel}", name = pname, channel = given)
            );
        }
        _ => {}
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peers_and_sides() {
        let items = vec![
            json!({"id": "a1", "name": "portátil-ana"}),
            json!({"id": "b2", "name": "portátil-bea"}),
        ];
        assert_eq!(
            find_peer(&items, "a1").unwrap()["name"],
            json!("portátil-ana")
        );
        assert_eq!(find_peer(&items, "BEA").unwrap()["id"], json!("b2"));
        assert!(find_peer(&items, "portátil").is_err());
        assert!(find_peer(&items, "zz").is_err());
        glisa_i18n::with_language(Some("es"), || {
            let c = side_choices();
            assert!(c.iter().any(|(k, v)| k == "izquierda" && *v == "left"));
            assert!(c.iter().any(|(k, _)| k == "left"));
        });
    }
}
