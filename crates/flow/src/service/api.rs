//! The `flow_*` API: same methods, parameters, results and error texts as
//! `FlowService.api` in the Python version. Runs in the caller's language
//! (`i18n::with_language`), so errors and notices are rendered for that client.

use std::time::{Duration, Instant};

use glisa_common::{ApiError, ApiResult};
use glisa_i18n::tr;
use serde_json::{Map, Value, json};

use super::engine::{is_pointer, kind_name, suggest_channel};
use super::{DISCOVERED_TTL, FlowService, ufw_active};
use crate::address::{format_address, parse_address};
use crate::backends::detect_here;
use crate::crypto::short_fingerprint;
use crate::text::render;
use crate::util::lock;
use crate::{CAP_TRANSFER, CLIPBOARD_MAX, MAX_FILES, PROTOCOL_VERSION, SIDES, pair_port};

fn err(e: impl Into<String>) -> ApiError {
    ApiError::new(e)
}

fn flag(p: &Value, key: &str) -> Result<bool, ApiError> {
    p.get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| err(tr!("Missing {key} (true or false)", key = key)))
}

fn req_str<'a>(p: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    match p.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() && s.chars().count() <= 128 => Ok(s),
        _ => Err(err(tr!("Missing {key}", key = key))),
    }
}

fn opt_str<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.chars().count() <= 128)
}

impl FlowService {
    pub(crate) fn handle(&self, m: &str, p: &Value) -> ApiResult {
        let empty = Value::Object(Map::new());
        let p = if p.is_object() { p } else { &empty };
        match m {
            "flow_status" => Ok(self.status()),
            "flow_enable" => {
                self.set_enabled(flag(p, "enabled")?)
                    .map_err(|e| err(e.render()))?;
                Ok(self.status())
            }
            "flow_peers" => {
                Ok(json!({"peers": self.peers_list(), "discovered": self.discovered_list()}))
            }
            "flow_discover" => {
                let wait = p.get("wait").and_then(Value::as_f64).unwrap_or(0.0);
                Ok(json!({"discovered": self.discover(wait)}))
            }
            "flow_pair_start" => self.pair_start().map_err(err),
            "flow_pair_request" => self
                .pair_request(opt_str(p, "peer"), opt_str(p, "address"))
                .map_err(err),
            "flow_pair_confirm" => self.pair_confirm(flag(p, "accept")?).map_err(err),
            "flow_pair_cancel" => {
                self.pair_cancel();
                Ok(json!(true))
            }
            "flow_remove_peer" => self
                .remove_peer(req_str(p, "peer")?)
                .map(Value::Bool)
                .map_err(err),
            "flow_set_peer_address" => self.set_peer_address(req_str(p, "peer")?, p.get("address")),
            "flow_set_layout" => {
                let side = match p.get("side") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(s)) if SIDES.contains(&s.as_str()) => Some(s.as_str()),
                    _ => return Err(err(tr!("Invalid position"))),
                };
                self.store
                    .set_side(req_str(p, "peer")?, side)
                    .map_err(err)?;
                self.emit_flow("layout");
                Ok(json!(self.store.layout()))
            }
            "flow_set_options" => {
                let mut opts = p.as_object().cloned().unwrap_or_default();
                opts.remove("id");
                let caps = self.caps();
                self.store.set_options(&opts).map_err(err)?;
                self.update_drag_watch();
                if self.caps() != caps {
                    self.renew_peer_channels();
                    if !self.store.options().drag_files {
                        lock(&self.st).incoming_drag = None;
                    }
                }
                self.emit_flow("options");
                Ok(self.store.options().to_json())
            }
            "flow_set_channel" => {
                let id = req_str(p, "device")?;
                let Some(dev) = self
                    .host()
                    .and_then(|h| h.flow_devices().into_iter().find(|d| d.id == id))
                else {
                    return Err(err(tr!("Device not found")));
                };
                let pid = req_str(p, "peer")?;
                let ch = match p.get("channel") {
                    None | Some(Value::Null) => None,
                    Some(Value::Number(n))
                        if n.is_u64()
                            && n.as_u64().unwrap_or(u64::MAX) < dev.hosts_count.max(1) as u64 =>
                    {
                        n.as_u64().map(|c| c as u8)
                    }
                    _ => return Err(err(tr!("Invalid channel"))),
                };
                self.store.set_channel(&dev.key, pid, ch).map_err(err)?;
                self.emit_flow("channels");
                Ok(json!(true))
            }
            "flow_events" => {
                let since = p
                    .get("since")
                    .and_then(Value::as_i64)
                    .filter(|_| p["since"].is_i64() || p["since"].is_u64())
                    .unwrap_or(0);
                let transfers = self.transfers_status();
                let files_backend = if self.running() {
                    self.files_kind()
                } else {
                    "none"
                };
                let clipboard_backend = self.clipboard_kind();
                let st = lock(&self.st);
                let events: Vec<Value> = st
                    .events
                    .iter()
                    .filter(|e| e.seq as i64 > since)
                    // rendered now: in the language of the client asking
                    .map(|e| json!({"seq": e.seq, "text": e.msg.render(), "kind": e.kind}))
                    .collect();
                Ok(json!({"seq": st.event_seq, "enabled": self.store.enabled(),
                          "clipboard_seq": st.gui_clip_seq, "clipboard_backend": clipboard_backend,
                          "files_backend": files_backend, "transfers": transfers, "events": events}))
            }
            "flow_clipboard_take" => {
                let st = lock(&self.st);
                let rich = st.gui_clip_rich.clone().unwrap_or_else(|| json!({}));
                Ok(json!({"seq": st.gui_clip_seq, "text": st.gui_clip_text,
                          "kind": rich.get("kind").cloned().unwrap_or(json!("text")),
                          "uris": rich.get("uris").cloned().filter(Value::is_array).unwrap_or(json!([])),
                          "image": rich.get("image").cloned().unwrap_or(Value::Null)}))
            }
            "flow_clipboard_offer" => {
                let Some(text) = p.get("text").and_then(Value::as_str) else {
                    return Err(err(tr!("Invalid text")));
                };
                let uris = match p.get("uris") {
                    None => Vec::new(),
                    Some(Value::Array(a))
                        if a.len() <= MAX_FILES
                            && a.iter()
                                .all(|u| u.as_str().is_some_and(|s| s.chars().count() <= 8192)) =>
                    {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    }
                    _ => return Err(err(tr!("Invalid text"))),
                };
                let mut st = lock(&self.st);
                st.gui_clip_offer = if text.len() <= CLIPBOARD_MAX {
                    text.to_string()
                } else {
                    String::new()
                };
                st.gui_clip_uris = uris;
                Ok(json!(true))
            }
            "flow_transfer_cancel" => Ok(json!(self.cancel_transfer(req_str(p, "transfer")?))),
            "flow_reset_identity" => {
                self.reset_identity().map_err(|e| err(e.render()))?;
                Ok(self.status())
            }
            "flow_simulate" => self.simulate(p),
            _ => Err(err(tr!("Unknown method: {method}", method = m))),
        }
    }

    /// Drive the fake backend (demo mode, tests, `glisa-ctl flow test-edge`).
    fn simulate(&self, p: &Value) -> ApiResult {
        let Some(b) = self.backend() else {
            return Err(err(tr!("Only available in demo mode")));
        };
        let Some(fake) = b.as_fake() else {
            return Err(err(tr!("Only available in demo mode")));
        };
        match p.get("op").and_then(Value::as_str) {
            Some("edge") => {
                let side = p
                    .get("side")
                    .and_then(Value::as_str)
                    .filter(|s| SIDES.contains(s));
                let t = match p.get("t") {
                    None => Some(0.5),
                    Some(v) if v.is_number() => v.as_f64(),
                    _ => None,
                };
                let (Some(side), Some(t)) = (side, t) else {
                    return Err(err(tr!("Invalid edge")));
                };
                fake.hit(side, t);
            }
            Some("away") => fake.away(),
            Some("ctrl") => fake.set_ctrl(p.get("held").and_then(Value::as_bool).unwrap_or(false)),
            _ => return Err(err(tr!("Unknown operation"))),
        }
        Ok(json!(true))
    }

    fn files_kind(&self) -> &'static str {
        self.backend()
            .and_then(|b| b.rich())
            .filter(|c| c.available())
            .map(|c| c.kind())
            .unwrap_or("gui")
    }

    pub fn discover(&self, wait: f64) -> Vec<Value> {
        if let Some(d) = lock(&self.discovery).as_ref() {
            d.query();
        }
        if wait > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(wait.min(3.0)));
        }
        self.discovered_list()
    }

    fn discovered_list(&self) -> Vec<Value> {
        let paired = self.store.peer_ids();
        let mut items: Vec<super::Discovered> = {
            let mut st = lock(&self.st);
            st.discovered
                .retain(|_, d| d.seen.elapsed() <= DISCOVERED_TTL);
            st.discovered.values().cloned().collect()
        };
        items.sort_by_key(|d| d.name.to_lowercase());
        items
            .into_iter()
            .map(|d| {
                json!({"id": d.id, "name": d.name, "address": format!("{}:{}", d.address.0, d.address.1),
                       "compatible": d.version == PROTOCOL_VERSION, "paired": paired.contains(&d.id)})
            })
            .collect()
    }

    pub fn peers_list(&self) -> Vec<Value> {
        let layout = self.store.layout();
        let mut peers: Vec<(String, crate::store::Peer)> = self.store.peers().into_iter().collect();
        peers.sort_by_key(|(_, p)| p.name.to_lowercase());
        let now = Instant::now();
        peers
            .into_iter()
            .map(|(pid, p)| {
                let link = self.link(&pid);
                let (online, identity) = link
                    .as_ref()
                    .map(|l| (l.online(), l.identity_error()))
                    .unwrap_or((false, false));
                let state = if online {
                    "online"
                } else if identity {
                    "identity"
                } else {
                    "offline"
                };
                // the address in use: the live connection's, else the first one tried
                let candidates: Vec<((String, u16), &str)> = match &link {
                    Some(l) => l.candidates(Some(&p)),
                    None => [
                        (p.manual_address.clone(), "manual"),
                        (p.address.clone(), "learned"),
                    ]
                    .into_iter()
                    .filter_map(|(a, s)| a.map(|a| (a, s)))
                    .collect(),
                };
                let live = if online {
                    link.as_ref().and_then(|l| l.addr())
                } else {
                    None
                };
                let (current, source) = match live {
                    Some(a) => {
                        let src = candidates
                            .iter()
                            .find(|(c, _)| *c == a)
                            .map(|(_, s)| *s)
                            .unwrap_or("");
                        (Some(a), src)
                    }
                    None => match candidates.first() {
                        Some((a, s)) => (Some(a.clone()), *s),
                        None => (None, ""),
                    },
                };
                let (offline_for, last_error) = match (&link, online) {
                    (Some(l), false) => {
                        let st = lock(&l.st);
                        (
                            now.duration_since(st.down_since).as_secs(),
                            st.error.clone(),
                        )
                    }
                    _ => (0, String::new()),
                };
                json!({"id": pid, "name": p.name, "online": online, "state": state,
                       "side": layout.get(&pid), "fingerprint": short_fingerprint(&p.pin),
                       "address": format_address(current.as_ref()), "address_source": source,
                       "manual_address": format_address(p.manual_address.as_ref()),
                       "learned_address": format_address(p.address.as_ref()),
                       "offline_for": offline_for, "last_error": last_error,
                       "files": link.is_some_and(|l| l.caps().contains(CAP_TRANSFER))})
            })
            .collect()
    }

    /// The address the user typed for a peer; empty / `auto` / null clears it.
    fn set_peer_address(&self, pid: &str, text: Option<&Value>) -> ApiResult {
        if self.store.peer(pid).is_none() {
            return Err(err(tr!("That computer isn't paired")));
        }
        let addr = match text {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => parse_address(s).map_err(err)?,
            Some(_) => return Err(err(tr!("Invalid address"))),
        };
        if self
            .store
            .set_manual_address(pid, addr.clone())
            .map_err(err)?
        {
            log::info!(
                "Flow: address of {} {}",
                self.store.peer(pid).map(|p| p.name).unwrap_or_default(),
                match &addr {
                    Some(a) => format!("set to {}", format_address(Some(a))),
                    None => "found automatically again".into(),
                }
            );
        }
        if let Some(link) = self.link(pid) {
            if addr.is_some() && link.online() && link.addr() != addr {
                link.close(); // reconnect where the user said
            }
            link.retry_now();
        }
        self.peers_changed();
        Ok(self
            .peers_list()
            .into_iter()
            .find(|p| p["id"] == pid)
            .unwrap_or(Value::Null))
    }

    pub fn devices_status(&self) -> Vec<Value> {
        let peers = self.store.peers();
        self.devices()
            .into_iter()
            .filter(|d| is_pointer(d.kind) || d.kind == glisa_common::DeviceKind::Keyboard)
            .map(|d| {
                let channels: Map<String, Value> = peers
                    .iter()
                    .map(|(pid, p)| {
                        (pid.clone(), json!({"channel": self.store.channel(&d.key, pid), "suggested": suggest_channel(&d, &p.name)}))
                    })
                    .collect();
                json!({"id": d.id, "name": d.name, "kind": kind_name(d.kind), "online": d.online,
                       "hosts": {"count": d.hosts_count, "current": d.current_host, "entries": d.host_entries},
                       "channels": channels})
            })
            .collect()
    }

    pub fn status(&self) -> Value {
        let b = self.backend();
        // Dragging files across the edge needs the edge catcher (KDE Plasma on Wayland):
        // known for sure while Flow runs, else from the desktop detected here.
        let drag_available = match &b {
            Some(b) => b.drag_catcher_available(),
            None if self.demo => true,
            None => detect_here().0 == Some("kwin"),
        };
        let (kind, state, message, ctrl, place) = match &b {
            Some(b) => {
                let (s, m) = b.state();
                (b.kind(), s, m, b.ctrl_available(), b.can_place())
            }
            None if self.demo || !matches!(self.backend_choice, super::BackendChoice::Auto) => {
                ("fake", "off".to_string(), None, true, true)
            }
            None => {
                let (k, reason) = detect_here();
                (
                    k.unwrap_or("none"),
                    if k.is_some() {
                        "off".into()
                    } else {
                        "unavailable".into()
                    },
                    reason,
                    k.is_some(),
                    k.is_some(),
                )
            }
        };
        let ident = self.store.id();
        let fingerprint = if !ident.is_empty() && self.store.cert_path.exists() {
            short_fingerprint(&self.store.pin())
        } else {
            String::new()
        };
        let udp = lock(&self.discovery)
            .as_ref()
            .map(|d| d.bound_port())
            .unwrap_or(self.net.udp());
        let running = self.running();
        let (error, last_switch, last_arrival, events_seq, clipboard_seq) = {
            let st = lock(&self.st);
            (
                render(&st.error),
                st.last_switch.clone(),
                st.last_arrival.clone(),
                st.event_seq,
                st.gui_clip_seq,
            )
        };
        let port = self.port();
        json!({
            "version": PROTOCOL_VERSION, "enabled": self.store.enabled(), "running": running,
            "demo": self.demo, "error": error,
            "backend": kind, "backend_state": state, "backend_message": render(&message),
            "available": kind != "none", "ctrl_available": ctrl, "can_place": place,
            "drag_available": drag_available,
            "clipboard_backend": if running { self.clipboard_kind() } else { "none" },
            "files_backend": if running { self.files_kind() } else { "none" },
            "transfers": self.transfers_status(),
            "staging": self.staging.root.to_string_lossy(),
            "id": ident, "name": self.store.hostname, "fingerprint": fingerprint,
            "ports": {"udp": udp, "tcp": port, "pairing": pair_port(port)},
            "firewall": {"ufw": !self.demo && ufw_active()},
            "options": self.store.options().to_json(),
            "peers": self.peers_list(),
            "discovered": self.discovered_list().into_iter().filter(|d| d["paired"] != true).collect::<Vec<_>>(),
            "pairing": self.pairing_status(),
            "devices": self.devices_status(),
            "screens": self.screens(),
            "last_switch": last_switch, "last_arrival": last_arrival,
            "events_seq": events_seq, "clipboard_seq": clipboard_seq,
        })
    }
}
