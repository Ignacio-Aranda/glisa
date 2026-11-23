//! Text helpers of the CLI: sizes, transfers, and yes/no answers.

use glisa_i18n::{tr, trc, trn};
use serde_json::Value;

/// 2048 -> "2 GB", 500 -> "500 MB" (the units read the same in every language).
pub fn size_text(mb: i64) -> String {
    if mb >= 1024 && mb % 1024 == 0 {
        format!("{} GB", mb / 1024)
    } else if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

/// 1.5 GB, 820 MB, 12 KB.
pub fn human_size(n: i64) -> String {
    for (unit, size) in [("GB", 1i64 << 30), ("MB", 1 << 20), ("KB", 1 << 10)] {
        if n >= size {
            let value = n as f64 / size as f64;
            return if value < 10.0 {
                format!("{value:.1} {unit}").replace(".0 ", " ")
            } else {
                format!("{value:.0} {unit}")
            };
        }
    }
    format!("{n} B")
}

fn int(t: &Value, key: &str) -> i64 {
    t.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn s<'a>(t: &'a Value, key: &str) -> &'a str {
    t.get(key).and_then(Value::as_str).unwrap_or("")
}

/// "3 files", "1 folder", "2 items", "an image" (mid-sentence).
pub fn transfer_what(t: &Value) -> String {
    if s(t, "kind") == "image" {
        return tr!("an image");
    }
    let n = int(t, "items");
    let folders = int(t, "top_folders");
    if n != 0 && folders == n {
        return trn!("{count} folder", "{count} folders", n, count = n);
    }
    if folders == 0 {
        return trn!("{count} file", "{count} files", n, count = n);
    }
    trn!("{count} item", "{count} items", n, count = n)
}

/// What a transfer notice says, in the user's language.
pub fn transfer_text(t: &Value) -> String {
    let what = transfer_what(t);
    let name = s(t, "name");
    let pct = int(t, "percent");
    let state = s(t, "state");
    let sending = s(t, "direction") == "send";
    let placed = t.get("placed").is_some_and(|p| match p {
        Value::String(x) => !x.is_empty(),
        Value::Bool(b) => *b,
        _ => false,
    });
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
        "done" if s(t, "placed") == "drop" => {
            tr!("Received {what} from {name}", what = what, name = name)
        }
        "done" => {
            if s(t, "kind") == "image" {
                if placed {
                    return trn!(
                        "Received {what} from {name} — paste it with Ctrl+V",
                        "Received {what} from {name} — paste them with Ctrl+V",
                        1,
                        what = what,
                        name = name
                    );
                }
                return trn!(
                    "Received {what} from {name}. It's in the Flow folder",
                    "Received {what} from {name}. They're in the Flow folder",
                    1,
                    what = what,
                    name = name
                );
            }
            let n = int(t, "items");
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

/// Lower case without accents ("Sí" -> "si").
pub fn plain(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

/// "y", "yes" and their translations ("s", "sí", "si").
pub fn is_yes(answer: &str) -> bool {
    let words = [
        "y".to_string(),
        "yes".to_string(),
        trc!("short answer", "y"),
        trc!("answer", "yes"),
    ];
    let a = plain(answer.trim());
    words.iter().any(|w| plain(w) == a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sizes() {
        assert_eq!(size_text(2048), "2 GB");
        assert_eq!(size_text(1536), "1.5 GB");
        assert_eq!(size_text(500), "500 MB");
        assert_eq!(human_size(1 << 30), "1 GB");
        assert_eq!(human_size(1536 << 10), "1.5 MB");
        assert_eq!(human_size(820 << 20), "820 MB");
        assert_eq!(human_size(12), "12 B");
    }

    #[test]
    fn transfers_and_answers() {
        glisa_i18n::with_language(Some("en"), || {
            let t = json!({"kind": "files", "items": 3, "top_folders": 0, "state": "sending",
                           "direction": "send", "name": "laptop", "percent": 40});
            assert_eq!(transfer_text(&t), "Sending 3 files to laptop… 40%");
            let dropped = json!({"kind": "files", "items": 2, "top_folders": 0, "state": "done",
                                 "direction": "receive", "name": "laptop", "placed": "drop"});
            assert_eq!(transfer_text(&dropped), "Received 2 files from laptop");
            assert!(is_yes("Y") && is_yes("yes") && !is_yes("n") && !is_yes(""));
        });
        glisa_i18n::with_language(Some("es"), || {
            assert!(is_yes("s") && is_yes("Sí") && is_yes("si"));
            let dropped = json!({"kind": "files", "items": 2, "top_folders": 0, "state": "done",
                                 "direction": "receive", "name": "laptop", "placed": "drop"});
            assert_eq!(transfer_text(&dropped), "Recibido de laptop: 2 archivos");
        });
    }
}
