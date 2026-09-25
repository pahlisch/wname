//! Keeping bands out of the way while Hyprland animates.
//!
//! Hyprland reports where a window is going, not where it is drawn. When a
//! window slides to a new spot, or a workspace slides in, the band would get
//! there first and float over the wrong thing. So when a band moves or comes
//! back for one of those reasons, it stays hidden for as long as the
//! animation runs. A move that happens while the pointer moves too is a drag,
//! which is not animated, so it is followed at once.

use crate::layout::{Hidden, Rect};
use serde::Deserialize;
use std::time::Duration;

/// How long each kind of animation runs, zero when it is off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Durations {
    pub window_move: Duration,
    pub workspace_in: Duration,
    pub special_in: Duration,
    pub special_out: Duration,
}

#[derive(Deserialize)]
struct Node {
    name: String,
    #[serde(default)]
    overridden: bool,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    speed: f64,
}

/// The node an animation takes its settings from when not overridden.
fn parent(name: &str) -> Option<&'static str> {
    Some(match name {
        "windowsIn" | "windowsOut" | "windowsMove" => "windows",
        "workspacesIn" | "workspacesOut" | "specialWorkspace" => "workspaces",
        "specialWorkspaceIn" | "specialWorkspaceOut" => "specialWorkspace",
        "windows" | "workspaces" | "layers" | "fade" | "border" | "borderangle" => "global",
        _ => return None,
    })
}

impl Durations {
    /// From `j/animations` (a list of animation nodes, then a list of
    /// beziers) and the `animations:enabled` master switch.
    pub fn parse(animations_json: &str, master_enabled: bool) -> Self {
        if !master_enabled {
            return Self::default();
        }
        let Ok((nodes, _)) =
            serde_json::from_str::<(Vec<Node>, serde_json::Value)>(animations_json)
        else {
            return Self::default();
        };
        let lookup = |start: &str| -> Duration {
            let mut name = start;
            // Bounded walk up the tree, in case of a cycle in odd input.
            for _ in 0..8 {
                match nodes.iter().find(|n| n.name == name) {
                    Some(n) if n.overridden || n.name == "global" => {
                        if !n.enabled || !n.speed.is_finite() || n.speed <= 0.0 {
                            return Duration::ZERO;
                        }
                        // Speed is in tenths of a second.
                        return Duration::from_secs_f64((n.speed / 10.0).min(10.0));
                    }
                    _ => match parent(name) {
                        Some(p) => name = p,
                        None => return Duration::ZERO,
                    },
                }
            }
            Duration::ZERO
        };
        Durations {
            window_move: lookup("windowsMove"),
            workspace_in: lookup("workspacesIn"),
            special_in: lookup("specialWorkspaceIn"),
            special_out: lookup("specialWorkspaceOut"),
        }
    }
}

/// What the band was doing before this refresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Before {
    /// Never placed yet: a band that was just created.
    New,
    Hidden(Hidden),
    Shown(Rect),
}

/// How long to keep a band hidden as it goes to `next`.
pub fn hold(
    before: Before,
    next: Rect,
    onto_special: bool,
    pointer_moved: bool,
    d: &Durations,
) -> Duration {
    match before {
        Before::New => Duration::ZERO,
        Before::Hidden(Hidden::SpecialOpen) => d.special_out,
        Before::Hidden(Hidden::OtherWorkspace) if onto_special => d.special_in,
        Before::Hidden(Hidden::OtherWorkspace) => d.workspace_in,
        // Back from fullscreen or from under another fullscreen window: the
        // window resizes back into place.
        Before::Hidden(Hidden::Fullscreen) if !pointer_moved => d.window_move,
        Before::Hidden(_) => Duration::ZERO,
        Before::Shown(prev) if prev != next && !pointer_moved => d.window_move,
        Before::Shown(_) => Duration::ZERO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = r#"[[
        {"name":"global","overridden":true,"bezier":"default","enabled":true,"speed":10.00,"style":""},
        {"name":"windows","overridden":true,"bezier":"easeOutQuint","enabled":true,"speed":3.79,"style":""},
        {"name":"windowsIn","overridden":true,"bezier":"easeOutQuint","enabled":true,"speed":4.10,"style":"popin 87%"},
        {"name":"windowsOut","overridden":true,"bezier":"linear","enabled":true,"speed":1.49,"style":"popin 87%"},
        {"name":"windowsMove","overridden":false,"bezier":"","enabled":true,"speed":0.00,"style":""},
        {"name":"workspaces","overridden":true,"bezier":"default","enabled":false,"speed":1.00,"style":""},
        {"name":"workspacesIn","overridden":false,"bezier":"","enabled":true,"speed":0.00,"style":""},
        {"name":"workspacesOut","overridden":false,"bezier":"","enabled":true,"speed":0.00,"style":""},
        {"name":"specialWorkspace","overridden":true,"bezier":"easeOutQuint","enabled":true,"speed":3.00,"style":"slidevert"},
        {"name":"specialWorkspaceIn","overridden":false,"bezier":"","enabled":true,"speed":0.00,"style":""},
        {"name":"specialWorkspaceOut","overridden":false,"bezier":"","enabled":true,"speed":0.00,"style":""}
    ], [{"name":"quick"}]]"#;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn close(a: Duration, b: Duration) -> bool {
        a.abs_diff(b) < ms(2)
    }

    #[test]
    fn inherits_through_the_tree() {
        let d = Durations::parse(REAL, true);
        assert!(close(d.window_move, ms(379)), "{d:?}");
        // workspaces is overridden and disabled, so workspacesIn is off.
        assert_eq!(d.workspace_in, Duration::ZERO);
        // specialWorkspace overrides the disabled workspaces node.
        assert!(close(d.special_in, ms(300)), "{d:?}");
        assert!(close(d.special_out, ms(300)), "{d:?}");
    }

    #[test]
    fn master_switch_turns_everything_off() {
        assert_eq!(Durations::parse(REAL, false), Durations::default());
    }

    #[test]
    fn falls_back_to_global() {
        let json = r#"[[{"name":"global","overridden":true,"enabled":true,"speed":5}], []]"#;
        let d = Durations::parse(json, true);
        assert_eq!(d.window_move, ms(500));
        assert_eq!(d.special_in, ms(500));
    }

    #[test]
    fn bad_input_means_no_holds() {
        for bad in [
            "",
            "{}",
            "[]",
            "[[{\"name\":\"global\",\"speed\":\"x\"}],[]]",
            "[[{\"name\":\"windowsMove\",\"overridden\":false}],[]]",
        ] {
            assert_eq!(Durations::parse(bad, true), Durations::default(), "{bad}");
        }
    }

    #[test]
    fn absurd_speeds_are_capped() {
        let json = r#"[[{"name":"global","overridden":true,"enabled":true,"speed":1e9}], []]"#;
        assert_eq!(
            Durations::parse(json, true).window_move,
            Duration::from_secs(10)
        );
    }

    fn rect(x: i32) -> Rect {
        Rect {
            x,
            y: 0,
            w: 100,
            h: 24,
        }
    }

    #[test]
    fn hold_rules() {
        let d = Durations {
            window_move: ms(1),
            workspace_in: ms(2),
            special_in: ms(3),
            special_out: ms(4),
        };
        let r = rect(0);
        assert_eq!(hold(Before::New, r, false, false, &d), Duration::ZERO);
        assert_eq!(
            hold(Before::Hidden(Hidden::OtherWorkspace), r, false, false, &d),
            ms(2)
        );
        assert_eq!(
            hold(Before::Hidden(Hidden::OtherWorkspace), r, true, false, &d),
            ms(3)
        );
        assert_eq!(
            hold(Before::Hidden(Hidden::SpecialOpen), r, false, false, &d),
            ms(4)
        );
        assert_eq!(
            hold(Before::Hidden(Hidden::Fullscreen), r, false, false, &d),
            ms(1)
        );
        assert_eq!(
            hold(Before::Hidden(Hidden::Fullscreen), r, false, true, &d),
            Duration::ZERO
        );
        assert_eq!(
            hold(Before::Hidden(Hidden::Covered), r, false, false, &d),
            Duration::ZERO
        );
        assert_eq!(
            hold(Before::Hidden(Hidden::Unmapped), r, false, false, &d),
            Duration::ZERO
        );
        assert_eq!(hold(Before::Shown(r), r, false, false, &d), Duration::ZERO);
        assert_eq!(hold(Before::Shown(rect(5)), r, false, false, &d), ms(1));
        // Dragging: the pointer moved with the window.
        assert_eq!(
            hold(Before::Shown(rect(5)), r, false, true, &d),
            Duration::ZERO
        );
    }
}
