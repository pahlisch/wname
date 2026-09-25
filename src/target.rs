//! Which window a bare `wname -n` means.
//!
//! Not simply the focused one: an agent running `wname` in a terminal may do
//! so while you are typing in another window. So the window is the one whose
//! process started this `wname`, found by walking up the parent processes.
//! Only when no ancestor owns a window (a keybinding, a detached tmux server)
//! does the focused window stand in.

use crate::hypr::Client;
use std::fs;

#[derive(Debug, PartialEq)]
pub enum Owner<'a> {
    /// The window this process runs in.
    Window(&'a Client),
    /// One process owns several windows (a terminal server, a browser) and
    /// none of them has focus, so there is no telling which is meant.
    Ambiguous { pid: i64, windows: Vec<&'a Client> },
    /// No ancestor owns a window.
    None,
}

/// The window owned by the nearest process in `ancestors` (nearest first).
/// A process with several windows resolves to the focused one if it is
/// among them.
pub fn owning_window<'a>(
    ancestors: &[i64],
    clients: &'a [Client],
    focused: Option<&str>,
) -> Owner<'a> {
    for &pid in ancestors {
        if pid <= 1 {
            break;
        }
        let windows: Vec<&Client> = clients.iter().filter(|c| c.pid == pid).collect();
        match windows.len() {
            0 => continue,
            1 => return Owner::Window(windows[0]),
            _ => {
                if let Some(f) = windows.iter().find(|c| Some(c.address.as_str()) == focused) {
                    return Owner::Window(f);
                }
                return Owner::Ambiguous { pid, windows };
            }
        }
    }
    Owner::None
}

/// Parent process of `pid`, from /proc.
fn parent_of(pid: i64) -> Option<i64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may contain spaces or ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// This process's ancestors, parent first, up to but not including init.
pub fn ancestors() -> Vec<i64> {
    let mut out = Vec::new();
    let mut pid = i64::from(std::process::id());
    // Bounded, in case /proc hands back a loop.
    for _ in 0..64 {
        match parent_of(pid) {
            Some(parent) if parent > 1 => {
                out.push(parent);
                pid = parent;
            }
            _ => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(address: &str, pid: i64) -> Client {
        serde_json::from_value(serde_json::json!({
            "address": address, "pid": pid, "at": [0, 0], "size": [100, 100]
        }))
        .unwrap()
    }

    #[test]
    fn nearest_ancestor_with_a_window_wins() {
        let clients = vec![client("0xa", 100), client("0xb", 200)];
        // wname <- bash(300) <- claude(301) <- bash(302) <- foot(200)
        assert_eq!(
            owning_window(&[300, 301, 302, 200, 50], &clients, Some("0xa")),
            Owner::Window(&clients[1])
        );
    }

    #[test]
    fn focus_does_not_matter_for_a_single_window() {
        let clients = vec![client("0xa", 100), client("0xb", 200)];
        assert_eq!(
            owning_window(&[200], &clients, Some("0xa")),
            Owner::Window(&clients[1])
        );
        assert_eq!(
            owning_window(&[200], &clients, None),
            Owner::Window(&clients[1])
        );
    }

    #[test]
    fn no_window_among_ancestors() {
        let clients = vec![client("0xa", 100)];
        assert_eq!(
            owning_window(&[300, 301], &clients, Some("0xa")),
            Owner::None
        );
        assert_eq!(owning_window(&[], &clients, Some("0xa")), Owner::None);
    }

    #[test]
    fn stops_at_init() {
        // A window claiming pid 1 must not match: everything descends from it.
        let clients = vec![client("0xa", 1)];
        assert_eq!(owning_window(&[1], &clients, None), Owner::None);
        let clients = vec![client("0xa", 5)];
        assert_eq!(owning_window(&[1, 5], &clients, None), Owner::None);
    }

    #[test]
    fn shared_process_prefers_its_focused_window() {
        let clients = vec![client("0xa", 100), client("0xb", 100), client("0xc", 200)];
        assert_eq!(
            owning_window(&[100], &clients, Some("0xb")),
            Owner::Window(&clients[1])
        );
    }

    #[test]
    fn shared_process_without_focus_is_ambiguous() {
        let clients = vec![client("0xa", 100), client("0xb", 100), client("0xc", 200)];
        match owning_window(&[100, 200], &clients, Some("0xc")) {
            Owner::Ambiguous { pid, windows } => {
                assert_eq!(pid, 100);
                assert_eq!(windows.len(), 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn ancestors_of_this_process() {
        let a = ancestors();
        assert!(!a.is_empty(), "the test runner has a parent");
        assert_eq!(a[0], i64::from(std::os::unix::process::parent_id()));
        assert!(a.iter().all(|&p| p > 1));
    }

    #[test]
    fn parent_parsing_survives_odd_names() {
        // /proc/self/stat of this test binary; its name has no spaces, but
        // the parser must skip to the last ')'.
        let me = i64::from(std::process::id());
        assert_eq!(
            parent_of(me),
            Some(i64::from(std::os::unix::process::parent_id()))
        );
        assert_eq!(parent_of(-5), None);
    }
}
