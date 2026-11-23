//! `glisa-ctl`: command-line access to the Glisa service (scriptable, handy for testing).
//! Output and help follow the system language (GLISA_LANG, LANGUAGE, LC_ALL, LC_MESSAGES,
//! LANG); JSON output is never translated.
//!
//! It talks to the service over its IPC endpoint (see `glisa_service::ipc`), except
//! `probe`, which reads the hardware directly.

use std::io::{self, Write};

use glisa_i18n::tr;
use glisa_service::{Client, ClientError};
use serde_json::{Value, json};

pub mod args;
pub mod flow;
pub mod interrupt;
pub mod probe;
pub mod text;

use args::{ParseError, Spec, flag, opt, pos};

/// The program's name: `<app dir>-ctl`.
pub fn prog() -> String {
    format!("{}-ctl", glisa_common::APP_DIR)
}

/// Where the CLI writes and how it asks questions (a line, or `None` at end of input).
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub ask: &'a mut dyn FnMut(&str) -> Option<String>,
}

pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The id of the device called `ident` (id, name or model; else the only partial
/// match); the first device when `None`.
pub fn pick(c: &mut Client, ident: Option<&str>) -> Result<String, ClientError> {
    let devs = c.call("devices", Value::Null)?;
    let devs = devs.as_array().cloned().unwrap_or_default();
    let Some(first) = devs.first() else {
        return Err(ClientError::Remote(tr!("There are no devices")));
    };
    let Some(ident) = ident else {
        return Ok(s(first, "id").to_string());
    };
    if let Some(d) = devs
        .iter()
        .find(|d| [s(d, "id"), s(d, "name"), s(d, "model")].contains(&ident))
    {
        return Ok(s(d, "id").to_string());
    }
    let low = ident.to_lowercase();
    if let Some(d) = devs.iter().find(|d| {
        ["name", "model"]
            .iter()
            .any(|k| s(d, k).to_lowercase().contains(&low))
    }) {
        return Ok(s(d, "id").to_string());
    }
    Err(ClientError::Remote(tr!(
        "Can't find “{name}”",
        name = ident
    )))
}

fn device_opt() -> args::Opt {
    opt(
        "device",
        "--device",
        Some("-d"),
        tr!("DEVICE"),
        String::new(),
    )
}

/// The top-level commands.
pub fn specs() -> Vec<Spec> {
    let simple = |name, help| Spec {
        name,
        help,
        positionals: vec![],
        options: vec![],
    };
    vec![
        simple("status", tr!("state of the service")),
        simple(
            "probe",
            tr!("talk to the hardware directly, without the service (diagnostics)"),
        ),
        simple("list", tr!("list the devices")),
        Spec {
            name: "info",
            help: tr!("everything about a device (JSON)"),
            positionals: vec![pos("device", tr!("DEVICE")).optional()],
            options: vec![],
        },
        Spec {
            name: "dpi",
            help: tr!("set the pointer speed (DPI)"),
            positionals: vec![pos("value", "DPI".into())],
            options: vec![device_opt()],
        },
        Spec {
            name: "set",
            help: tr!("change a setting, e.g. set wheel_inverted true"),
            positionals: vec![pos("key", tr!("KEY")), pos("value", tr!("VALUE"))],
            options: vec![device_opt()],
        },
        Spec {
            name: "button",
            help: tr!("assign an action: button 0x53 '{\"type\":\"keys\",\"combo\":\"Ctrl+C\"}'"),
            positionals: vec![pos("cid", "CID".into()), pos("action", tr!("ACTION"))],
            options: vec![device_opt()],
        },
        Spec {
            name: "host",
            help: tr!("switch the device to another computer (Easy-Switch channel)"),
            positionals: vec![pos("number", "1..3".into())],
            options: vec![device_opt()],
        },
        Spec {
            name: "reset",
            help: tr!("restore the device's factory buttons and settings"),
            positionals: vec![pos("device", tr!("DEVICE")).optional()],
            options: vec![],
        },
        Spec {
            name: "rename",
            help: tr!("choose the name Glisa shows for a device (\"\" for the model's)"),
            positionals: vec![pos("name", tr!("NAME"))],
            options: vec![device_opt()],
        },
        Spec {
            name: "forget",
            help: tr!("remove a device from Glisa and delete its settings"),
            positionals: vec![pos("device", tr!("DEVICE"))],
            options: vec![flag(
                "yes",
                "--yes",
                Some("-y"),
                tr!("don't ask for confirmation"),
            )],
        },
        Spec {
            name: "flow",
            help: tr!("use the mouse on another computer running Glisa"),
            positionals: vec![pos("command", tr!("COMMAND"))],
            options: vec![],
        },
    ]
}

fn top_help() -> String {
    let mut out = format!(
        "{}: {} [--socket {}] {} …\n\n{}\n\n  --socket {:<13} {}\n\n",
        tr!("usage"),
        prog(),
        "SOCKET",
        tr!("COMMAND"),
        tr!("Command-line access to the Glisa service"),
        "SOCKET",
        tr!("socket of the service (default: the session's)")
    );
    for s in specs() {
        out.push_str(&format!("  {:<22} {}\n", s.name, s.help));
    }
    out
}

fn sub_help(prefix: &str, specs: &[Spec], help: &str) -> String {
    let mut out = format!(
        "{}: {prefix} {} …\n\n{help}\n\n",
        tr!("usage"),
        tr!("COMMAND")
    );
    for s in specs {
        out.push_str(&format!("  {:<22} {}\n", s.name, s.help));
    }
    out
}

fn usage_error(io: &mut Io, usage: &str, msg: &str) -> u8 {
    let _ = writeln!(io.err, "{}: {usage}", tr!("usage"));
    let _ = writeln!(
        io.err,
        "{}: {}",
        prog(),
        tr!("error: {message}", message = msg)
    );
    2
}

fn print_json(out: &mut dyn Write, v: &Value) {
    let _ = writeln!(
        out,
        "{}",
        serde_json::to_string_pretty(v).unwrap_or_default()
    );
}

/// Runs the CLI with `argv` (without the program name); returns the exit code.
pub fn run(argv: &[String], io: &mut Io) -> u8 {
    let prog = prog();
    let mut socket: Option<String> = None;
    let mut i = 0;
    while let Some(a) = argv.get(i) {
        if a == "-h" || a == "--help" {
            let _ = write!(io.out, "{}", top_help());
            return 0;
        } else if a == "--socket" {
            let Some(v) = argv.get(i + 1) else {
                return usage_error(
                    io,
                    &format!("{prog} [--socket SOCKET] COMMAND"),
                    &tr!(
                        "{option} needs a value ({value})",
                        option = "--socket",
                        value = "SOCKET"
                    ),
                );
            };
            socket = Some(v.clone());
            i += 2;
        } else if let Some(v) = a.strip_prefix("--socket=") {
            socket = Some(v.to_string());
            i += 1;
        } else if a.starts_with('-') {
            return usage_error(
                io,
                &format!("{prog} [--socket SOCKET] COMMAND"),
                &tr!("unrecognized argument: {arg}", arg = a),
            );
        } else {
            break;
        }
    }
    let Some(cmd) = argv.get(i) else {
        return usage_error(
            io,
            &format!("{prog} [--socket SOCKET] COMMAND"),
            &tr!("missing argument: {name}", name = tr!("COMMAND")),
        );
    };
    let rest = &argv[i + 1..];
    let all = specs();
    let Some(spec) = all.iter().find(|s| s.name == cmd) else {
        return usage_error(
            io,
            &format!("{prog} [--socket SOCKET] COMMAND"),
            &tr!("unknown command: {command}", command = cmd),
        );
    };
    // `flow` has subcommands of its own.
    let (flow_cmd, spec_used, parse_args): (Option<String>, Spec, &[String]) = if cmd == "flow" {
        let flow_specs = flow::specs();
        let Some(sub) = rest.first() else {
            return usage_error(
                io,
                &format!("{prog} flow COMMAND"),
                &tr!("missing argument: {name}", name = tr!("COMMAND")),
            );
        };
        if sub == "-h" || sub == "--help" {
            let _ = write!(
                io.out,
                "{}",
                sub_help(&format!("{prog} flow"), &flow_specs, &spec.help)
            );
            return 0;
        }
        let Some(pos) = flow_specs.iter().position(|s| s.name == sub) else {
            return usage_error(
                io,
                &format!("{prog} flow COMMAND"),
                &tr!("unknown command: {command}", command = sub),
            );
        };
        let mut flow_specs = flow_specs;
        (Some(sub.clone()), flow_specs.swap_remove(pos), &rest[1..])
    } else {
        let idx = all.iter().position(|s| s.name == cmd).unwrap_or(0);
        let mut all = all;
        (None, all.swap_remove(idx), rest)
    };
    let prog = if flow_cmd.is_some() {
        format!("{prog} flow")
    } else {
        prog.clone()
    };
    let a = match spec_used.parse(parse_args) {
        Ok(a) => a,
        Err(ParseError::Help) => {
            let _ = write!(io.out, "{}", spec_used.help_text(&prog));
            return 0;
        }
        Err(ParseError::Bad(msg)) => return usage_error(io, &spec_used.usage(&prog), &msg),
    };
    if cmd == "probe" {
        return probe::run(io.out).unwrap_or(1);
    }
    let mut c = match Client::new(socket.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(io.err, "{}", tr!("error: {message}", message = e));
            return 1;
        }
    };
    let result = match &flow_cmd {
        Some(sub) => flow::command(&mut c, sub, &a, io),
        None => command(&mut c, cmd, &a, io),
    };
    c.close();
    match result {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(io.err, "{}", tr!("error: {message}", message = e));
            1
        }
    }
}

fn command(c: &mut Client, cmd: &str, a: &args::Parsed, io: &mut Io) -> Result<u8, ClientError> {
    let bad = |m: String| ClientError::Remote(m);
    let out = match cmd {
        "status" => c.call("status", Value::Null)?,
        "list" => {
            for d in c
                .call("devices", Value::Null)?
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                let bat = match d.get("battery").filter(|b| !b.is_null()) {
                    Some(b) if b.get("percent").is_some_and(truthy) => {
                        format!("{}%", b["percent"])
                    }
                    Some(b) => s(b, "level").to_string(),
                    None => "-".to_string(),
                };
                let model = match s(&d, "model") {
                    "" => s(&d, "name"),
                    m => m,
                };
                let _ = writeln!(
                    io.out,
                    "{:<28} {:<24} {:<24} {:<14} {bat}",
                    s(&d, "id"),
                    s(&d, "name"),
                    model,
                    flow::connection(d.get("online").is_some_and(truthy))
                );
            }
            return Ok(0);
        }
        "rename" => {
            let id = pick(c, a.get("device"))?;
            let dev = c.call(
                "rename",
                json!({"id": id, "name": a.get("name").unwrap_or("")}),
            )?;
            let name = s(&dev, "name");
            let model = match s(&dev, "model") {
                "" => name,
                m => m,
            };
            let msg = if name == model {
                tr!("{model} uses its model name again", model = model)
            } else {
                tr!("{model} is now called “{name}”", model = model, name = name)
            };
            let _ = writeln!(io.out, "{msg}");
            return Ok(0);
        }
        "forget" => return forget(c, a.get("device").unwrap_or(""), a.flag("yes"), io),
        "info" => {
            let id = pick(c, a.get("device"))?;
            c.call("device", json!({"id": id}))?
        }
        "dpi" => {
            let v: i64 = a
                .get("value")
                .and_then(|v| v.trim().parse().ok())
                .ok_or_else(|| bad(tr!("Invalid parameters")))?;
            let id = pick(c, a.get("device"))?;
            c.call("set", json!({"id": id, "key": "dpi", "value": v}))?
        }
        "set" => {
            let raw = a.get("value").unwrap_or("");
            let value = serde_json::from_str::<Value>(raw).unwrap_or_else(|_| json!(raw));
            let id = pick(c, a.get("device"))?;
            c.call(
                "set",
                json!({"id": id, "key": a.get("key"), "value": value}),
            )?
        }
        "button" => {
            let raw = a.get("action").unwrap_or("");
            let action = if raw.is_empty() || raw == "default" {
                Value::Null
            } else {
                serde_json::from_str::<Value>(raw).map_err(|e| bad(e.to_string()))?
            };
            let cid = args::parse_int_auto(a.get("cid").unwrap_or(""))
                .ok_or_else(|| bad(tr!("Invalid parameters")))?;
            let id = pick(c, a.get("device"))?;
            c.call(
                "set_button",
                json!({"id": id, "cid": cid, "action": action}),
            )?
        }
        "host" => {
            let n: i64 = a
                .get("number")
                .and_then(|v| v.trim().parse().ok())
                .ok_or_else(|| bad(tr!("Invalid channel")))?;
            let id = pick(c, a.get("device"))?;
            c.call("set_host", json!({"id": id, "host": n - 1}))?
        }
        "reset" => {
            let id = pick(c, a.get("device"))?;
            c.call("reset", json!({"id": id}))?
        }
        _ => return Ok(2),
    };
    print_json(io.out, &out);
    Ok(0)
}

fn forget(c: &mut Client, ident: &str, yes: bool, io: &mut Io) -> Result<u8, ClientError> {
    let id = pick(c, Some(ident))?;
    let devs = c.call("devices", Value::Null)?;
    let name = devs
        .as_array()
        .and_then(|l| l.iter().find(|d| s(d, "id") == id))
        .map_or(ident.to_string(), |d| s(d, "name").to_string());
    if !yes {
        let _ = writeln!(
            io.out,
            "{}",
            tr!(
                "Glisa will forget {name}: its buttons, pointer settings and name are deleted, and if it's connected its factory settings are restored. It comes back with default settings the next time it connects.",
                name = name
            )
        );
        let answer = (io.ask)(&tr!("Remove it? [y/N] ")).unwrap_or_default();
        if !text::is_yes(&answer) {
            let _ = writeln!(io.out, "{}", tr!("Nothing was removed"));
            return Ok(1);
        }
    }
    c.call("forget", json!({"id": id}))?;
    let _ = writeln!(io.out, "{}", tr!("{name} removed", name = name));
    Ok(0)
}

/// Asks on the terminal: prints `prompt`, reads one line (`None` at end of input or on
/// Ctrl+C).
pub fn ask_terminal(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let r = io::stdin()
            .read_line(&mut line)
            .ok()
            .filter(|n| *n > 0)
            .map(|_| line);
        let _ = tx.send(r);
    });
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(r) => return r.map(|l| l.trim_end_matches(['\n', '\r']).to_string()),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if interrupt::triggered() => {
                return None;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return None,
        }
    }
}
