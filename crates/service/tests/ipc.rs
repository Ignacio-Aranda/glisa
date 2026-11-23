//! The IPC server and client: private socket, peer-uid checks both ways, request
//! validation, languages.

#![cfg(unix)]

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use glisa_service::{Client, ClientError, NoHid, Service, ServiceOptions};
use serde_json::{Value, json};

fn my_uid() -> u32 {
    // SAFETY: getuid has no preconditions.
    unsafe { libc::getuid() }
}

fn started(dir: &TmpDir) -> (Arc<Service>, String) {
    let sock = dir.path("s").to_string_lossy().into_owned();
    let s = Service::new(ServiceOptions {
        config_path: Some(dir.path("c.json")),
        simulate: true,
        dry_run_input: true,
        ipc: Some(sock.clone()),
        ..Default::default()
    })
    .unwrap();
    s.set_hid_backend(Arc::new(NoHid)); // never the real hardware
    s.start().unwrap();
    (s, sock)
}

fn raw(sock: &str, line: &[u8]) -> Value {
    let mut c = UnixStream::connect(sock).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c.write_all(line).unwrap();
    let mut r = BufReader::new(c);
    let mut out = String::new();
    r.read_line(&mut out).unwrap();
    serde_json::from_str(&out).unwrap()
}

#[test]
fn client_and_server_talk_json_lines() {
    let dir = TmpDir::new("ipc");
    let (s, sock) = started(&dir);
    assert_eq!(
        std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let mut c = Client::new(Some(&sock)).unwrap();
    assert_eq!(
        c.call("ping", json!({})).unwrap()["version"],
        json!(glisa_common::VERSION)
    );
    let devs = c.call("devices", Value::Null).unwrap();
    let ids: Vec<&str> = devs
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "b031-lift",
            "b034-wireless_mouse_mx-sim",
            "b35b-wireless_keyboard_k-sim"
        ]
    );
    // Errors come back in the client's language.
    glisa_i18n::with_language(Some("es"), || {
        let e = c.call("device", json!({"id": "nope"})).unwrap_err();
        assert_eq!(e, ClientError::Remote("Dispositivo no encontrado".into()));
    });
    glisa_i18n::with_language(Some("en"), || {
        let e = c.call("device", json!({"id": "nope"})).unwrap_err();
        assert_eq!(e.to_string(), "Device not found");
    });
    // The answer carries the request's id.
    let r = raw(&sock, b"{\"id\": 42, \"method\": \"ping\"}\n");
    assert_eq!(r["id"], json!(42));
    s.stop();
    assert!(
        !std::path::Path::new(&sock).exists(),
        "the socket goes away on stop"
    );
}

#[test]
fn malformed_requests_are_answered_not_fatal() {
    let dir = TmpDir::new("ipcbad");
    let (s, sock) = started(&dir);
    let r = raw(&sock, b"not json\n");
    assert_eq!(r["id"], Value::Null);
    assert!(r["error"].is_string());
    let r = raw(&sock, b"[1, 2]\n");
    assert!(r["error"].is_string());
    let r = raw(
        &sock,
        b"{\"id\": 1, \"method\": \"devices\", \"params\": [1], \"lang\": \"es\"}\n",
    );
    assert_eq!(r["error"], json!("Parámetros no válidos"));
    let r = raw(&sock, b"{\"id\": 2, \"method\": 7, \"params\": {}}\n");
    assert!(r["error"].is_string());
    // A line over the limit closes the connection without an answer.
    let mut c = UnixStream::connect(&sock).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let big = vec![b'x'; glisa_service::ipc::MAX_LINE + 10];
    let _ = c.write_all(&big);
    let mut buf = Vec::new();
    let _ = c.read_to_end(&mut buf);
    assert!(buf.is_empty());
    // The server still works.
    assert!(
        Client::new(Some(&sock))
            .unwrap()
            .call("ping", Value::Null)
            .is_ok()
    );
    s.stop();
}

#[test]
fn a_second_service_does_not_steal_the_socket() {
    let dir = TmpDir::new("ipc2");
    let (s, sock) = started(&dir);
    let other = Service::new(ServiceOptions {
        config_path: Some(dir.path("c2.json")),
        dry_run_input: true,
        ipc: Some(sock.clone()),
        ..Default::default()
    })
    .unwrap();
    other.set_hid_backend(Arc::new(NoHid));
    let e = glisa_i18n::with_language(Some("en"), || other.start().unwrap_err());
    assert!(e.to_string().contains("already listening"), "{e}");
    other.stop();
    assert!(
        Client::new(Some(&sock))
            .unwrap()
            .call("ping", Value::Null)
            .is_ok()
    );
    s.stop();
}

#[test]
fn stale_socket_is_replaced_but_other_files_are_not() {
    let dir = TmpDir::new("ipc3");
    let sock = dir.path("s");
    drop(UnixListener::bind(&sock).unwrap()); // a socket nobody listens on any more
    let (s, sock_s) = started(&dir);
    assert!(
        Client::new(Some(&sock_s))
            .unwrap()
            .call("ping", Value::Null)
            .is_ok()
    );
    s.stop();
    let file = dir.path("f");
    std::fs::write(&file, "keep me").unwrap();
    let s2 = Service::new(ServiceOptions {
        config_path: Some(dir.path("c3.json")),
        dry_run_input: true,
        ipc: Some(file.to_string_lossy().into_owned()),
        ..Default::default()
    })
    .unwrap();
    s2.set_hid_backend(Arc::new(NoHid));
    assert!(s2.start().is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep me");
    s2.stop();
}

#[test]
fn server_refuses_clients_of_another_user() {
    let dir = TmpDir::new("ipc4");
    let s = service(dir.path("c.json"), true);
    let sock = dir.path("s").to_string_lossy().into_owned();
    // The server expects "another user": we are that other user's peer.
    let mut server =
        glisa_service::ipc::serve_for_uid(Arc::downgrade(&s), &sock, my_uid() + 1).unwrap();
    let r = raw(&sock, b"{\"id\": 1, \"method\": \"devices\"}\n");
    assert_eq!(r["id"], Value::Null);
    assert!(r["error"].is_string());
    assert!(r.get("result").is_none());
    server.stop();
    s.stop();
}

#[test]
fn client_refuses_a_service_of_another_user() {
    let dir = TmpDir::new("ipc5");
    let path = dir.path("s");
    let srv = UnixListener::bind(&path).unwrap();
    let got: Arc<Mutex<Vec<u8>>> = Arc::default();
    let got2 = got.clone();
    let th = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut c, _) = srv.accept().unwrap();
            c.set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let mut buf = [0u8; 4096];
            let n = c.read(&mut buf).unwrap_or(0);
            got2.lock().unwrap().extend_from_slice(&buf[..n]);
            if n > 0 {
                let _ = c.write_all(b"{\"id\": 1, \"result\": \"pong\"}\n");
            }
        }
    });
    let p = path.to_string_lossy().into_owned();
    let e = glisa_i18n::with_language(Some("es"), || {
        Client::new(Some(&p))
            .unwrap()
            .expect_uid(my_uid() + 1)
            .call("ping", json!({"text": "secreto"}))
            .unwrap_err()
    });
    assert!(e.to_string().contains("no pertenece"), "{e}");
    // A service of this user: fine.
    let mut same = Client::new(Some(&p)).unwrap();
    assert_eq!(same.call("ping", Value::Null).unwrap(), json!("pong"));
    th.join().unwrap();
    let all = got.lock().unwrap();
    assert!(
        !String::from_utf8_lossy(&all).contains("secreto"),
        "nothing was sent to the impostor"
    );
}

#[test]
fn client_without_service() {
    let dir = TmpDir::new("ipc6");
    let p = dir.path("none").to_string_lossy().into_owned();
    let e = glisa_i18n::with_language(Some("en"), || {
        Client::new(Some(&p))
            .unwrap()
            .call("ping", Value::Null)
            .unwrap_err()
    });
    assert!(
        e.to_string()
            .starts_with("Can't connect to the Glisa service ("),
        "{e}"
    );
    assert!(!e.to_string().contains("os error"));
}
