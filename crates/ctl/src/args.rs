//! A small command-line parser (subcommands, positionals, options), so help and errors
//! are in the user's language and no parser crate is needed.

use std::collections::HashMap;

use glisa_i18n::tr;

pub struct Pos {
    pub key: &'static str,
    pub metavar: String,
    pub required: bool,
    pub choices: Option<Vec<String>>,
    pub help: Option<String>,
}

pub struct Opt {
    pub key: &'static str,
    pub long: &'static str,
    pub short: Option<&'static str>,
    /// None: a flag.
    pub metavar: Option<String>,
    pub choices: Option<Vec<String>>,
    pub help: String,
}

pub struct Spec {
    pub name: &'static str,
    pub help: String,
    pub positionals: Vec<Pos>,
    pub options: Vec<Opt>,
}

pub fn pos(key: &'static str, metavar: String) -> Pos {
    Pos {
        key,
        metavar,
        required: true,
        choices: None,
        help: None,
    }
}

impl Pos {
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }
    pub fn choices(mut self, c: Vec<String>) -> Self {
        self.choices = Some(c);
        self
    }
    pub fn help(mut self, h: String) -> Self {
        self.help = Some(h);
        self
    }
}

pub fn flag(
    key: &'static str,
    long: &'static str,
    short: Option<&'static str>,
    help: String,
) -> Opt {
    Opt {
        key,
        long,
        short,
        metavar: None,
        choices: None,
        help,
    }
}

pub fn opt(
    key: &'static str,
    long: &'static str,
    short: Option<&'static str>,
    metavar: String,
    help: String,
) -> Opt {
    Opt {
        key,
        long,
        short,
        metavar: Some(metavar),
        choices: None,
        help,
    }
}

impl Opt {
    pub fn choices(mut self, c: Vec<String>) -> Self {
        self.choices = Some(c);
        self
    }
}

#[derive(Debug, Default)]
pub struct Parsed {
    values: HashMap<&'static str, String>,
    flags: Vec<&'static str>,
}

impl Parsed {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
    pub fn flag(&self, key: &str) -> bool {
        self.flags.contains(&key)
    }
}

/// What went wrong, or that help was asked for.
pub enum ParseError {
    Help,
    Bad(String),
}

impl Spec {
    pub fn usage(&self, prog: &str) -> String {
        let mut u = format!("{prog} {}", self.name);
        for o in &self.options {
            let name = o.short.unwrap_or(o.long);
            match &o.metavar {
                Some(m) => u.push_str(&format!(" [{name} {m}]")),
                None => u.push_str(&format!(" [{name}]")),
            }
        }
        for p in &self.positionals {
            if p.required {
                u.push_str(&format!(" {}", p.metavar));
            } else {
                u.push_str(&format!(" [{}]", p.metavar));
            }
        }
        u
    }

    pub fn help_text(&self, prog: &str) -> String {
        let mut out = format!("{}: {}\n\n{}\n", tr!("usage"), self.usage(prog), self.help);
        let mut rows: Vec<(String, String)> = Vec::new();
        for p in &self.positionals {
            let mut h = p.help.clone().unwrap_or_default();
            if let Some(c) = &p.choices {
                if !h.is_empty() {
                    h.push(' ');
                }
                h.push_str(&format!("{{{}}}", c.join(",")));
            }
            rows.push((p.metavar.clone(), h));
        }
        for o in &self.options {
            let mut names = String::new();
            if let Some(s) = o.short {
                names.push_str(s);
                names.push_str(", ");
            }
            names.push_str(o.long);
            if let Some(m) = &o.metavar {
                names.push(' ');
                names.push_str(m);
            }
            let mut h = o.help.clone();
            if let Some(c) = &o.choices {
                h.push_str(&format!(" {{{}}}", c.join(",")));
            }
            rows.push((names, h));
        }
        if !rows.is_empty() {
            out.push('\n');
        }
        for (a, b) in rows {
            out.push_str(&format!("  {a:<22} {b}\n"));
        }
        out
    }

    pub fn parse(&self, args: &[String]) -> Result<Parsed, ParseError> {
        let mut parsed = Parsed::default();
        let mut positionals: Vec<&String> = Vec::new();
        let mut i = 0;
        let mut only_positionals = false;
        while i < args.len() {
            let a = &args[i];
            i += 1;
            if only_positionals || !a.starts_with('-') || a == "-" {
                positionals.push(a);
                continue;
            }
            if a == "--" {
                only_positionals = true;
                continue;
            }
            if a == "-h" || a == "--help" {
                return Err(ParseError::Help);
            }
            let (name, inline) = match a.split_once('=') {
                Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
                _ => (a.as_str(), None),
            };
            let Some(o) = self
                .options
                .iter()
                .find(|o| o.long == name || o.short == Some(name))
            else {
                return Err(ParseError::Bad(tr!(
                    "unrecognized argument: {arg}",
                    arg = a
                )));
            };
            match &o.metavar {
                None => parsed.flags.push(o.key),
                Some(m) => {
                    let value = match inline {
                        Some(v) => v,
                        None => {
                            let Some(v) = args.get(i) else {
                                return Err(ParseError::Bad(tr!(
                                    "{option} needs a value ({value})",
                                    option = o.long,
                                    value = m
                                )));
                            };
                            i += 1;
                            v.clone()
                        }
                    };
                    if let Some(c) = &o.choices {
                        if !c.contains(&value) {
                            return Err(ParseError::Bad(tr!(
                                "invalid choice for {name}: {value} (choose from {choices})",
                                name = o.long,
                                value = value,
                                choices = c.join(", ")
                            )));
                        }
                    }
                    parsed.values.insert(o.key, value);
                }
            }
        }
        let mut it = positionals.into_iter();
        for p in &self.positionals {
            match it.next() {
                Some(v) => {
                    if let Some(c) = &p.choices {
                        if !c.contains(v) {
                            return Err(ParseError::Bad(tr!(
                                "invalid choice for {name}: {value} (choose from {choices})",
                                name = p.metavar,
                                value = v,
                                choices = c.join(", ")
                            )));
                        }
                    }
                    parsed.values.insert(p.key, v.clone());
                }
                None if p.required => {
                    return Err(ParseError::Bad(tr!(
                        "missing argument: {name}",
                        name = p.metavar
                    )));
                }
                None => {}
            }
        }
        if let Some(extra) = it.next() {
            return Err(ParseError::Bad(tr!(
                "unrecognized argument: {arg}",
                arg = extra
            )));
        }
        Ok(parsed)
    }
}

/// Python's `int(text, 0)`: "83", "0x53", "0o123", "0b101", with an optional sign.
pub fn parse_int_auto(text: &str) -> Option<i64> {
    let t = text.trim().replace('_', "");
    let (neg, body) = match t.strip_prefix('-') {
        Some(b) => (true, b.to_string()),
        None => (false, t.strip_prefix('+').unwrap_or(&t).to_string()),
    };
    let lower = body.to_ascii_lowercase();
    let v = if let Some(h) = lower.strip_prefix("0x") {
        i64::from_str_radix(h, 16).ok()?
    } else if let Some(o) = lower.strip_prefix("0o") {
        i64::from_str_radix(o, 8).ok()?
    } else if let Some(b) = lower.strip_prefix("0b") {
        i64::from_str_radix(b, 2).ok()?
    } else {
        if lower.len() > 1 && lower.starts_with('0') && lower.chars().any(|c| c != '0') {
            return None; // Python refuses "053"
        }
        lower.parse().ok()?
    };
    Some(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses() {
        let spec = Spec {
            name: "rename",
            help: "h".into(),
            positionals: vec![pos("name", "NAME".into())],
            options: vec![
                opt(
                    "device",
                    "--device",
                    Some("-d"),
                    "DEVICE".into(),
                    "d".into(),
                ),
                flag("yes", "--yes", Some("-y"), "y".into()),
            ],
        };
        let p = spec
            .parse(&s(&["-d", "LIFT", "Office  mouse", "-y"]))
            .ok()
            .unwrap();
        assert_eq!(p.get("device"), Some("LIFT"));
        assert_eq!(p.get("name"), Some("Office  mouse"));
        assert!(p.flag("yes"));
        let p = spec.parse(&s(&["--device=mx", ""])).ok().unwrap();
        assert_eq!((p.get("device"), p.get("name")), (Some("mx"), Some("")));
        assert!(matches!(spec.parse(&s(&[])), Err(ParseError::Bad(_))));
        assert!(matches!(
            spec.parse(&s(&["a", "b"])),
            Err(ParseError::Bad(_))
        ));
        assert!(matches!(
            spec.parse(&s(&["--nope", "a"])),
            Err(ParseError::Bad(_))
        ));
        assert!(matches!(spec.parse(&s(&["-h"])), Err(ParseError::Help)));
        assert!(spec.help_text(&crate::prog()).contains("--device DEVICE"));
    }

    #[test]
    fn ints() {
        assert_eq!(parse_int_auto("0x53"), Some(0x53));
        assert_eq!(parse_int_auto("83"), Some(83));
        assert_eq!(parse_int_auto("0"), Some(0));
        assert_eq!(parse_int_auto("053"), None);
        assert_eq!(parse_int_auto("-0b11"), Some(-3));
        assert_eq!(parse_int_auto("x"), None);
    }
}
