//! User-facing text that is translated when it is read, not when it is made.
//!
//! Flow produces messages in background threads (notices, the edge-detection state, start-up
//! errors) that clients fetch later. Freezing them at creation time would answer every
//! client in the service's language, so they are kept as an English msgid plus arguments
//! ([`Text`]) and rendered inside the request being answered (which runs in
//! `i18n::with_language(<client's language>)`).
//!
//! The helpers at the end describe a transfer for the GUI and the CLI.

use glisa_i18n::{n_, tr, trn};
use serde_json::Value;

pub use glisa_i18n::{Text, TextArg};

/// Text that is not translated (an OS error, a name): rendered as it is.
pub fn plain(text: impl Into<String>) -> Text {
    Text::new("{text}").arg("text", text.into())
}

/// Rendered in this thread's language; "" for nothing.
pub fn render(msg: &Option<Text>) -> String {
    msg.as_ref().map(Text::render).unwrap_or_default()
}

/// 2048 → "2 GB", 500 → "500 MB" (the units read the same in every language).
pub fn size_text(mb: u64) -> String {
    if mb >= 1024 && mb % 1024 == 0 {
        format!("{} GB", mb / 1024)
    } else if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

/// 1.5 GB, 820 MB, 12 KB (decimal-looking units over binary sizes, as file managers show).
pub fn human_size(n: u64) -> String {
    for (unit, size) in [("GB", 1u64 << 30), ("MB", 1 << 20), ("KB", 1 << 10)] {
        if n >= size {
            let value = n as f64 / size as f64;
            return if value < 10.0 {
                format!("{value:.1} {unit}").replace(".0 ", " ")
            } else {
                format!("{} {unit}", py_round0(value))
            };
        }
    }
    format!("{n} B")
}

/// Python's `f"{value:.0f}"` (round half to even).
fn py_round0(v: f64) -> i64 {
    v.round_ties_even() as i64
}

fn int(t: &Value, key: &str) -> u64 {
    t.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn string<'a>(t: &'a Value, key: &str) -> &'a str {
    t.get(key).and_then(Value::as_str).unwrap_or("")
}

/// "3 files", "1 folder", "2 items", "an image" (mid-sentence), from a transfer snapshot.
pub fn transfer_what(t: &Value) -> String {
    if string(t, "kind") == "image" {
        return tr!("an image");
    }
    let n = int(t, "items");
    let folders = int(t, "top_folders");
    if n > 0 && folders == n {
        return trn!("{count} folder", "{count} folders", n, count = n);
    }
    if folders == 0 {
        return trn!("{count} file", "{count} files", n, count = n);
    }
    trn!("{count} item", "{count} items", n, count = n)
}

/// What a transfer notice says, in this thread's language.
pub fn transfer_text(t: &Value) -> String {
    let what = transfer_what(t);
    let name = string(t, "name");
    let pct = int(t, "percent");
    let state = string(t, "state");
    let sending = string(t, "direction") == "send";
    let placed = !string(t, "placed").is_empty();
    match state {
        "preparing" => tr!("Getting {what} ready for {name}…", what = what, name = name),
        "waiting" | "sending" => tr!(
            "Sending {what} to {name}… {percent}%",
            what = what,
            name = name,
            percent = pct
        ),
        "receiving" => tr!(
            "Receiving {what} from {name}… {percent}%",
            what = what,
            name = name,
            percent = pct
        ),
        "done" if sending => tr!("Sent {what} to {name}", what = what, name = name),
        "done" if string(t, "placed") == "drop" => {
            // Dragged here and dropped where the user chose: nothing to paste.
            tr!("Received {what} from {name}", what = what, name = name)
        }
        "done" => {
            // An image is one item: the plural entries' singular form.
            let n = if string(t, "kind") == "image" {
                1
            } else {
                int(t, "items")
            };
            if placed {
                trn!(
                    "Received {what} from {name} — paste it with Ctrl+V",
                    "Received {what} from {name} — paste them with Ctrl+V",
                    n,
                    what = what,
                    name = name
                )
            } else {
                trn!(
                    "Received {what} from {name}. It's in the Flow folder",
                    "Received {what} from {name}. They're in the Flow folder",
                    n,
                    what = what,
                    name = name
                )
            }
        }
        "cancelled" if sending => tr!("Cancelled: {what} for {name}", what = what, name = name),
        "cancelled" => tr!("Cancelled: {what} from {name}", what = what, name = name),
        _ if sending => tr!("Didn't arrive: {what} for {name}", what = what, name = name),
        _ => tr!(
            "Didn't arrive: {what} from {name}",
            what = what,
            name = name
        ),
    }
}

/// The banner title every "not available" message starts with (in every language).
pub const UNAVAILABLE: &str = n_!("Flow is not available on this desktop");

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sizes() {
        assert_eq!(size_text(2048), "2 GB");
        assert_eq!(size_text(1536), "1.5 GB");
        assert_eq!(size_text(500), "500 MB");
        assert_eq!(human_size(1 << 20), "1 MB");
        assert_eq!(human_size(1536 << 10), "1.5 MB");
        assert_eq!(human_size(50 << 20), "50 MB");
        assert_eq!(human_size(12), "12 B");
    }

    #[test]
    fn transfer_texts() {
        glisa_i18n::with_language(Some("en"), || {
            let t = json!({"kind": "files", "items": 2, "top_folders": 1, "name": "equipo-a",
                           "state": "done", "direction": "receive", "placed": "clipboard"});
            assert_eq!(
                transfer_text(&t),
                "Received 2 items from equipo-a — paste them with Ctrl+V"
            );
            let t = json!({"kind": "image", "name": "equipo-a", "state": "done",
                           "direction": "receive", "placed": "clipboard"});
            assert_eq!(
                transfer_text(&t),
                "Received an image from equipo-a — paste it with Ctrl+V"
            );
            let t = json!({"kind": "files", "items": 1, "top_folders": 0, "name": "b",
                           "state": "cancelled", "direction": "receive"});
            assert_eq!(transfer_text(&t), "Cancelled: 1 file from b");
        });
        assert_eq!(
            glisa_i18n::with_language(Some("en"), || plain("{x} y").render()),
            "{x} y"
        );
    }
}
