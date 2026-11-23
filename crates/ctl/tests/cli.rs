//! The CLI against a real service over its socket. Simulated devices, temporary directories
//! only.

#![cfg(unix)]

use std::sync::Arc;

use glisa_service::{NoHid, Service, ServiceOptions};

struct Env {
    dir: std::path::PathBuf,
    sock: String,
    s: Arc<Service>,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.s.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn env(tag: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("lo-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("s").to_string_lossy().into_owned();
    let s = Service::new(ServiceOptions {
        config_path: Some(dir.join("c.json")),
        dry_run_input: true,
        ipc: Some(sock.clone()),
        ..Default::default()
    })
    .unwrap();
    s.set_hid_backend(Arc::new(NoHid)); // never the real hardware
    s.start().unwrap();
    s.add_simulated("lift");
    s.add_simulated("mx");
    Env { dir, sock, s }
}

fn run(e: &Env, args: &[&str], answer: Option<&str>) -> (u8, String, String) {
    let mut argv = vec!["--socket".to_string(), e.sock.clone()];
    argv.extend(args.iter().map(|a| a.to_string()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let answer = answer.map(str::to_string);
    let mut ask = move |prompt: &str| {
        let _ = prompt;
        answer.clone()
    };
    let code = {
        let mut io = glisa_ctl::Io {
            out: &mut out,
            err: &mut err,
            ask: &mut ask,
        };
        glisa_ctl::run(&argv, &mut io)
    };
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

#[test]
fn rename_and_list() {
    let e = env("rename");
    glisa_i18n::with_language(Some("en"), || {
        let (code, out, _) = run(&e, &["rename", "-d", "LIFT", "Office  mouse"], None);
        assert_eq!((code, out.trim()), (0, "LIFT is now called “Office mouse”"));
        let (_, out, _) = run(&e, &["list"], None);
        let line = out.lines().find(|l| l.starts_with("b031-lift")).unwrap();
        assert!(line.contains("Office mouse"));
        assert!(line.replace("Office mouse", "").contains("LIFT")); // the model, too
        assert!(line.contains("78%"));
        let (code, out, _) = run(&e, &["rename", "-d", "LIFT", ""], None); // by its model name still
        assert_eq!((code, out.trim()), (0, "LIFT uses its model name again"));
        let long = "z".repeat(41);
        let (code, _, err) = run(&e, &["rename", "-d", "mx", &long], None);
        assert_eq!(code, 1);
        assert_eq!(
            err.trim(),
            "error: The name can't be longer than 40 characters"
        );
    });
    glisa_i18n::with_language(Some("es"), || {
        let (_, out, _) = run(&e, &["rename", "-d", "mx", "Sofá"], None);
        assert_eq!(out.trim(), "Wireless Mouse MX-Sim ahora se llama «Sofá»");
    });
}

#[test]
fn forget_asks_first() {
    let e = env("forget");
    e.s.device("b031-lift")
        .unwrap()
        .set("dpi", &serde_json::json!(2400))
        .unwrap();
    glisa_i18n::with_language(Some("es"), || {
        let (code, out, _) = run(&e, &["forget", "LIFT"], Some("n"));
        assert_eq!(code, 1);
        assert!(out.contains("No se ha quitado nada"), "{out}");
        assert!(e.s.device("b031-lift").is_some());
        let (code, out, _) = run(&e, &["forget", "LIFT"], Some("s"));
        assert_eq!(code, 0);
        assert!(out.contains("LIFT quitado"), "{out}");
        assert!(e.s.device("b031-lift").is_none());
    });
    glisa_i18n::with_language(Some("en"), || {
        let (code, out, _) = run(&e, &["forget", "--yes", "mx"], None);
        assert_eq!((code, out.trim()), (0, "Wireless Mouse MX-Sim removed"));
    });
    assert!(e.s.devices().is_empty());
}

#[test]
fn json_commands_and_errors() {
    let e = env("json");
    glisa_i18n::with_language(Some("en"), || {
        let (code, out, _) = run(&e, &["dpi", "1600", "-d", "LIFT"], None);
        assert_eq!((code, out.trim()), (0, "1600"));
        let (code, out, _) = run(&e, &["set", "wheel_inverted", "true"], None);
        assert_eq!((code, out.trim()), (0, "true"));
        let (code, out, _) = run(
            &e,
            &[
                "button",
                "0x53",
                r#"{"type":"keys","combo":"Ctrl+C"}"#,
                "-d",
                "lift",
            ],
            None,
        );
        assert_eq!((code, out.trim()), (0, "true"));
        let (code, out, _) = run(&e, &["info", "LIFT"], None);
        assert_eq!(code, 0);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["settings"]["dpi"], serde_json::json!(1600));
        assert_eq!(
            v["controls"][1]["action"]["combo"],
            serde_json::json!("Ctrl+C")
        );
        let (code, out, _) = run(&e, &["button", "83", "default"], None);
        assert_eq!((code, out.trim()), (0, "true"));
        let (code, _, err) = run(&e, &["host", "9"], None);
        assert_eq!((code, err.trim()), (1, "error: Invalid channel"));
        let (code, _, err) = run(&e, &["info", "nothing-like-this"], None);
        assert_eq!(
            (code, err.trim()),
            (1, "error: Can't find “nothing-like-this”")
        );
        let (code, out, _) = run(&e, &["status"], None);
        assert_eq!(code, 0);
        assert!(out.contains("\"permissions\""));
        // Usage errors: exit code 2, nothing sent.
        assert_eq!(run(&e, &["dpi"], None).0, 2);
        assert_eq!(run(&e, &["frob"], None).0, 2);
        assert_eq!(run(&e, &["flow", "layout", "x", "sideways"], None).0, 2);
        let (code, out, _) = run(&e, &["--help"], None);
        assert_eq!(code, 0);
        assert!(out.contains("rename") && out.contains("flow"));
        // No Flow attached to this service: the error comes back.
        let (code, _, err) = run(&e, &["flow", "status"], None);
        assert_eq!((code, err.trim()), (1, "error: Flow isn't available"));
    });
}

#[test]
fn no_service() {
    let dir = std::env::temp_dir().join(format!("lo-cli-none-{}", std::process::id()));
    let argv = vec![
        "--socket".to_string(),
        dir.join("nothing").to_string_lossy().into_owned(),
        "list".to_string(),
    ];
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut ask = |_: &str| None;
    let mut io = glisa_ctl::Io {
        out: &mut out,
        err: &mut err,
        ask: &mut ask,
    };
    let code = glisa_i18n::with_language(Some("en"), || glisa_ctl::run(&argv, &mut io));
    assert_eq!(code, 1);
    assert!(
        String::from_utf8(err)
            .unwrap()
            .starts_with("error: Can't connect to the Glisa service")
    );
}

#[test]
fn flow_drag_option() {
    // A Flow that is never turned on (no ports): only its options are used.
    let e = env("flowdrag");
    let host: Arc<dyn glisa_common::FlowHost> = e.s.clone();
    let flow = glisa_flow::FlowService::new(
        Arc::downgrade(&host),
        Arc::new(|_| {}),
        glisa_flow::FlowOptions {
            store_dir: Some(e.dir.join("flow")),
            staging_dir: Some(e.dir.join("received")),
            demo: true,
            hostname: Some("equipo-a".into()),
            ..Default::default()
        },
    );
    e.s.set_flow(flow.clone());
    glisa_i18n::with_language(Some("en"), || {
        let (_, out, _) = run(&e, &["flow", "status"], None);
        assert!(out.contains("files dragged across the edge: yes"), "{out}");
        let (code, _, err) = run(&e, &["flow", "options", "--drag", "off"], None);
        assert_eq!(code, 0, "{err}");
        assert!(!flow.store.options().drag_files);
        let (_, out, _) = run(&e, &["flow", "status"], None);
        assert!(out.contains("files dragged across the edge: no"), "{out}");
        assert_eq!(run(&e, &["flow", "options", "--drag", "maybe"], None).0, 2);
        let (code, _, _) = run(&e, &["flow", "options", "--drag", "on"], None);
        assert_eq!(code, 0);
        assert!(flow.store.options().drag_files);
        let (_, out, _) = run(&e, &["flow", "options", "--help"], None);
        assert!(out.contains("--drag"), "{out}");
    });
}
