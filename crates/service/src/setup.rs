//! Linux system integration: the udev rule that gives the logged-in user access to Logitech
//! hidraw nodes and `/dev/uinput`, and its installation through polkit.
//!
//! Security (SECURITY.md): the rule text is checked to do nothing but `TAG+="uaccess"`
//! (and the uinput static node), and it travels inside the pkexec command line, so root
//! never reads a file that the user, or anything running as the user, could swap after
//! the password was typed; the polkit dialog's details show exactly what is installed.

use std::path::Path;

use glisa_common::APP_DIR;

/// File name of our rule (must sort before 73-seat-late.rules): `70-<app>.rules`.
pub fn rules_name() -> String {
    format!("70-{APP_DIR}.rules")
}

/// Where the in-app setup installs it.
pub fn rules_target() -> String {
    format!("/etc/udev/rules.d/{}", rules_name())
}

/// Where the .deb/.rpm packages install it.
pub fn rules_packaged() -> String {
    format!("/usr/lib/udev/rules.d/{}", rules_name())
}

/// Where the Python version (LinOptions) installed the same rule. Fixed: it grants
/// the same access, so an installation made by the old version still counts.
pub const LEGACY_RULES_TARGET: &str = "/etc/udev/rules.d/70-linoptions.rules";

/// The rule. The .deb/.rpm install the same file (app/src-tauri/linux/70-glisa.rules).
pub const RULES: &str = include_str!("../data/70-glisa.rules");

pub fn rules_installed() -> bool {
    Path::new(&rules_target()).exists()
        || Path::new(&rules_packaged()).exists()
        || Path::new(LEGACY_RULES_TARGET).exists()
}

/// Whether this process may write to /dev/uinput.
pub fn uinput_accessible() -> bool {
    #[cfg(target_os = "linux")]
    {
        let path = std::ffi::CString::new("/dev/uinput").expect("no NUL");
        // SAFETY: path is a valid NUL-terminated string.
        unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

fn which(program: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

/// Whether polkit's pkexec is available.
pub fn can_elevate() -> bool {
    cfg!(target_os = "linux") && which("pkexec").is_some()
}

/// One comma-separated item of a rule line: match keys (`==` or `!=`) from a short
/// list, and as the only actions the `uaccess` tag and the static uinput node. Never
/// RUN, PROGRAM, IMPORT, MODE, OWNER, or an ENV assignment…
fn rule_item_ok(item: &str) -> bool {
    if item == r#"TAG+="uaccess""# || item == r#"OPTIONS+="static_node=uinput""# {
        return true;
    }
    [
        "KERNEL",
        "SUBSYSTEM",
        "KERNELS",
        "ATTRS{idVendor}",
        "ATTRS{idProduct}",
        "ATTRS{id/vendor}",
        "ENV{ID_INPUT_MOUSE}",
        "ENV{ID_INPUT_KEYBOARD}",
    ]
    .iter()
    .any(|key| {
        item.strip_prefix(key)
            .and_then(|r| r.strip_prefix("==\"").or_else(|| r.strip_prefix("!=\"")))
            .and_then(|r| r.strip_suffix('"'))
            .is_some_and(|value| !value.contains('"'))
    })
}

/// Checks that `text` only grants `uaccess`; the error names the offending line.
pub fn check_rules(text: &str) -> Result<(), String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let items: Vec<&str> = line.split(',').map(str::trim).collect();
        if !items.iter().any(|i| i.starts_with("TAG+=")) || !items.iter().all(|i| rule_item_ok(i)) {
            let shown: String = line.chars().take(80).collect();
            return Err(format!("unexpected udev rule: {shown}"));
        }
    }
    Ok(())
}

/// The rule to install, checked.
pub fn rules_text() -> Result<&'static str, String> {
    check_rules(RULES).map(|()| RULES)
}

/// argv installing `rules` as root through polkit (one password prompt). Exit code 0 =
/// success, 126 = the user dismissed the prompt, 127 = not authorised.
pub fn install_rules_command_for(rules: &str) -> Result<Vec<String>, String> {
    check_rules(rules)?;
    let script = format!(
        "umask 022 && \
         printf '%s' \"$1\" > {t}.new && mv -f {t}.new {t} && \
         echo uinput > /etc/modules-load.d/{APP_DIR}-uinput.conf; \
         modprobe uinput 2>/dev/null; \
         udevadm control --reload-rules && \
         udevadm trigger --subsystem-match=hidraw --subsystem-match=misc --subsystem-match=input",
        t = rules_target()
    );
    Ok(vec![
        "pkexec".into(),
        "/bin/sh".into(),
        "-c".into(),
        script,
        format!("{APP_DIR}-setup"),
        rules.into(),
    ])
}

pub fn install_rules_command() -> Result<Vec<String>, String> {
    install_rules_command_for(rules_text()?)
}

/// Runs [`install_rules_command`] and waits (the user types a password). Returns the
/// exit code.
pub fn install_rules() -> std::io::Result<i32> {
    let argv = install_rules_command().map_err(std::io::Error::other)?;
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .status()?;
    Ok(status.code().unwrap_or(-1))
}

/// One command to paste into a terminal when polkit can't be used: it writes the rule
/// itself (no file to download), loads uinput and applies the rule to the devices that are
/// already connected.
pub fn manual_install_hint() -> String {
    let lines: Vec<String> = RULES
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| format!("'{l}'"))
        .collect();
    format!(
        "printf '%s\\n' {} | sudo tee {t} >/dev/null && \\\n         echo uinput | sudo tee /etc/modules-load.d/{APP_DIR}-uinput.conf >/dev/null; \\\n         sudo modprobe uinput; sudo udevadm control --reload-rules && \\\n         sudo udevadm trigger --subsystem-match=hidraw --subsystem-match=misc --subsystem-match=input",
        lines.join(" "),
        t = rules_target()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_rules_are_accepted() {
        assert!(RULES.contains("uaccess"));
        assert!(RULES.contains("0005:046D"));
        assert_eq!(rules_text(), Ok(RULES));
        let packaged = include_str!("../../../app/src-tauri/linux/70-glisa.rules");
        assert_eq!(RULES, packaged, "keep the packaged copy in sync");
    }

    #[test]
    fn install_command_is_a_single_privileged_call_carrying_the_rules() {
        let cmd = install_rules_command().unwrap();
        assert_eq!(cmd[..3], ["pkexec", "/bin/sh", "-c"]);
        assert!(cmd[3].contains(&rules_target()));
        assert!(cmd[3].contains("udevadm control --reload-rules"));
        assert_eq!(cmd.last().unwrap(), RULES);
        assert!(!cmd[3].contains(".rules\" ") && !cmd[3].contains("data/"));
    }

    #[test]
    fn the_manual_command_carries_the_rule_itself() {
        // Single-quoted lines are safe: no rule line (comments aside) has a quote of that kind.
        assert!(
            RULES
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .all(|l| !l.contains('\''))
        );
        let cmd = manual_install_hint();
        for line in RULES
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            assert!(cmd.contains(&format!("'{line}'")), "{line}");
        }
        assert!(cmd.contains(&format!("sudo tee {}", rules_target())));
        assert!(cmd.contains("--subsystem-match=input"));
        assert!(!cmd.contains(".rules <"), "no file the user doesn't have");
        // Valid shell, as pasted.
        let ok = std::process::Command::new("sh")
            .args(["-n", "-c", &cmd])
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "{cmd}");
        // And it writes exactly the rule's lines (run with printf only, no sudo).
        let printf = cmd.split(" | sudo").next().unwrap();
        let out = std::process::Command::new("sh")
            .args(["-c", printf])
            .output()
            .unwrap();
        let written = String::from_utf8(out.stdout).unwrap();
        let expected: Vec<&str> = RULES
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        assert_eq!(written.lines().collect::<Vec<_>>(), expected);
        assert_eq!(check_rules(&written), Ok(()));
    }

    #[test]
    fn rules_that_do_more_than_uaccess_are_refused() {
        for extra in [
            r#"KERNEL=="hidraw*", RUN+="/bin/sh -c id""#,
            r#"KERNEL=="uinput", MODE="0666""#,
            r#"KERNEL=="hidraw*", TAG+="uaccess", PROGRAM="/tmp/x""#,
            r#"KERNEL=="hidraw*", TAG+="uaccess", ENV{X}="1""#,
            r#"KERNEL=="a"b", TAG+="uaccess""#,
            r#"KERNEL=="event*", ENV{ID_INPUT_MOUSE}="1", TAG+="uaccess""#,
            r#"KERNEL=="event*", ENV{ID_INPUT_TOUCHPAD}=="1", TAG+="uaccess""#,
        ] {
            let text = format!("{RULES}{extra}\n");
            assert!(install_rules_command_for(&text).is_err(), "{extra}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_file_writing_half_writes_exactly_the_rules() {
        // Runs the script's first part as us, into a temp dir, to check the quoting.
        let d = std::env::temp_dir().join(format!("lo-sec-rules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let target = d.join(rules_name());
        let cmd = install_rules_command().unwrap();
        let script = cmd[3]
            .split(" && echo uinput")
            .next()
            .unwrap()
            .replace(&rules_target(), target.to_str().unwrap());
        let tricky = format!("{RULES}# quotes ' \" $HOME `id` %s\n");
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", &script, "setup", &tricky])
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), tricky);
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
        assert!(!d.join(format!("{}.new", rules_name())).exists());
        std::fs::remove_dir_all(d).unwrap();
    }
}
