//! Where a band goes, or why it is hidden, from one Hyprland snapshot.
//!
//! The band is a layer surface on a monitor, not part of the window, so the
//! compositor will not hide it for us. Everything that would make the window
//! itself invisible has to be worked out here:
//!
//! * the window's workspace is not the one its monitor shows;
//! * a special workspace is open over it;
//! * it is fullscreen, or another window on its workspace is;
//! * it is a background tab in a group, or unmapped;
//! * another window is stacked above it across the band;
//! * it is partly or wholly off its monitor.
//!
//! Hyprland does not report stacking order, so it is approximated: special
//! workspace over normal, then fullscreen, pinned, floating, tiled, and within
//! a tier the most recently focused window is on top.

use crate::hypr::{Client, Monitor};
use serde::{Deserialize, Serialize};

/// A band narrower than this is not worth showing.
pub const MIN_WIDTH: i32 = 32;
/// When other windows cut into the band, the widest uncovered stretch must be
/// at least this share of the full band to stay up.
pub const MIN_UNCOVERED_SHARE: f64 = 0.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn of(c: &Client) -> Self {
        Rect {
            x: c.at[0],
            y: c.at[1],
            w: c.size[0],
            h: c.size[1],
        }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Hidden {
    Unmapped,
    OtherWorkspace,
    SpecialOpen,
    Fullscreen,
    NoMonitor,
    OffScreen,
    TooSmall,
    Covered,
    /// Waiting for Hyprland to finish animating the window into place.
    Animating,
}

impl Hidden {
    pub fn describe(self) -> &'static str {
        match self {
            Hidden::Unmapped => "window hidden",
            Hidden::OtherWorkspace => "on a workspace not in view",
            Hidden::SpecialOpen => "under a special workspace",
            Hidden::Fullscreen => "fullscreen",
            Hidden::NoMonitor => "no monitor",
            Hidden::OffScreen => "off screen",
            Hidden::TooSmall => "window too small",
            Hidden::Covered => "covered by another window",
            Hidden::Animating => "waiting for an animation",
        }
    }
}

/// A band on screen, in the monitor's layout coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub monitor: String,
    /// Relative to the monitor.
    pub rect: Rect,
    /// In global layout coordinates.
    pub global: Rect,
    pub margins: Margins,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Margins {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placement {
    /// The window no longer exists; the band should be dropped.
    Gone,
    Hidden(Hidden),
    Shown(Shown),
}

/// The monitor a window is on screen on, or why it is not.
pub fn showing<'a>(c: &Client, mons: &'a [Monitor]) -> Result<&'a Monitor, Hidden> {
    if !c.mapped || c.hidden || !c.visible {
        return Err(Hidden::Unmapped);
    }
    let ws = c.workspace.id;
    for m in mons.iter().filter(|m| !m.disabled) {
        let special = m.special_workspace.id;
        if special != 0 && ws == special {
            return Ok(m);
        }
        if ws == m.active_workspace.id {
            return if special != 0 {
                Err(Hidden::SpecialOpen)
            } else {
                Ok(m)
            };
        }
    }
    // A pinned window follows its monitor across workspaces, but a special
    // workspace still covers it.
    if c.pinned {
        let m = mons
            .iter()
            .find(|m| m.id == c.monitor && !m.disabled)
            .ok_or(Hidden::NoMonitor)?;
        return if m.special_workspace.id != 0 {
            Err(Hidden::SpecialOpen)
        } else {
            Ok(m)
        };
    }
    Err(Hidden::OtherWorkspace)
}

/// Approximate stacking order; bigger is closer to the viewer.
fn stack_key(c: &Client) -> (u8, i64) {
    let special = c.workspace.id < 0;
    let tier = if c.fullscreen > 0 {
        3
    } else if c.pinned {
        2
    } else if c.floating {
        1
    } else {
        0
    };
    (u8::from(special) * 10 + tier, -c.focus_history_id)
}

/// Longest stretch of `[start, end)` not covered by any of `cuts`.
pub fn widest_gap(start: i32, end: i32, cuts: &mut [(i32, i32)]) -> (i32, i32) {
    cuts.sort_unstable();
    let mut best = (start, start);
    let mut cursor = start;
    for &(a, b) in cuts.iter() {
        let (a, b) = (a.max(start), b.min(end));
        if b <= a {
            continue;
        }
        if a > cursor && a - cursor > best.1 - best.0 {
            best = (cursor, a);
        }
        cursor = cursor.max(b);
    }
    if end > cursor && end - cursor > best.1 - best.0 {
        best = (cursor, end);
    }
    best
}

pub fn place(address: &str, clients: &[Client], mons: &[Monitor], bar_h: i32) -> Placement {
    let Some(target) = clients.iter().find(|c| c.address == address) else {
        return Placement::Gone;
    };
    match place_client(target, clients, mons, bar_h) {
        Ok(shown) => Placement::Shown(shown),
        Err(why) => Placement::Hidden(why),
    }
}

fn place_client(
    t: &Client,
    clients: &[Client],
    mons: &[Monitor],
    bar_h: i32,
) -> Result<Shown, Hidden> {
    let mon = showing(t, mons)?;
    // True fullscreen is for video and games: stay out of the way.
    // Maximized (1) keeps the band.
    if t.fullscreen >= 2 {
        return Err(Hidden::Fullscreen);
    }
    let win = Rect::of(t);
    let (mw, mh) = mon.logical_size();
    let screen = Rect {
        x: mon.x,
        y: mon.y,
        w: mw,
        h: mh,
    };

    // Clip horizontally to the monitor. Vertically, slide the band down to
    // the top of the screen when the window's top edge is above it, so the
    // name stays readable, but never below the window's own bottom.
    let x0 = win.x.max(screen.x);
    let x1 = win.right().min(screen.right());
    let y0 = win.y.max(screen.y);
    if x1 <= x0 || y0 >= screen.bottom() || win.bottom() <= screen.y {
        return Err(Hidden::OffScreen);
    }
    if win.bottom().min(screen.bottom()) - y0 < bar_h {
        return Err(Hidden::TooSmall);
    }
    let full_width = x1 - x0;
    if full_width < MIN_WIDTH {
        return Err(Hidden::TooSmall);
    }

    let band = Rect {
        x: x0,
        y: y0,
        w: full_width,
        h: bar_h,
    };
    let mine = stack_key(t);
    let mut cuts: Vec<(i32, i32)> = clients
        .iter()
        .filter(|o| o.address != t.address && stack_key(o) > mine)
        .filter(|o| showing(o, mons).is_ok())
        .map(Rect::of)
        .filter(|r| r.overlaps(&band))
        .map(|r| (r.x, r.right()))
        .collect();
    let (gx0, gx1) = widest_gap(band.x, band.right(), &mut cuts);
    let width = gx1 - gx0;
    if width < MIN_WIDTH || f64::from(width) < f64::from(full_width) * MIN_UNCOVERED_SHARE {
        return Err(Hidden::Covered);
    }

    let rect = Rect {
        x: gx0 - screen.x,
        y: band.y - screen.y,
        w: width,
        h: bar_h,
    };
    let margins = Margins {
        left: rect.x,
        top: rect.y,
        right: mw - rect.right(),
        bottom: mh - rect.bottom(),
    };
    Ok(Shown {
        monitor: mon.name.clone(),
        global: Rect {
            x: gx0,
            y: band.y,
            w: width,
            h: bar_h,
        },
        rect,
        margins,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypr::WorkspaceRef;

    const BAR: i32 = 24;

    fn mon(id: i64, name: &str, x: i32, ws: i64) -> Monitor {
        Monitor {
            id,
            name: name.into(),
            x,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            transform: 0,
            active_workspace: WorkspaceRef {
                id: ws,
                name: ws.to_string(),
            },
            special_workspace: WorkspaceRef::default(),
            focused: id == 0,
            disabled: false,
        }
    }

    fn win(addr: &str, ws: i64, at: [i32; 2], size: [i32; 2], focus: i64) -> Client {
        Client {
            address: addr.into(),
            mapped: true,
            hidden: false,
            visible: true,
            at,
            size,
            workspace: WorkspaceRef {
                id: ws,
                name: ws.to_string(),
            },
            floating: false,
            pinned: false,
            monitor: 0,
            class: "foot".into(),
            title: addr.into(),
            fullscreen: 0,
            grouped: vec![],
            focus_history_id: focus,
            pid: 0,
        }
    }

    fn one_monitor() -> Vec<Monitor> {
        vec![mon(0, "DP-1", 0, 1)]
    }

    fn shown(p: Placement) -> Shown {
        match p {
            Placement::Shown(s) => s,
            other => panic!("expected shown, got {other:?}"),
        }
    }

    fn hidden(p: Placement) -> Hidden {
        match p {
            Placement::Hidden(h) => h,
            other => panic!("expected hidden, got {other:?}"),
        }
    }

    #[test]
    fn band_sits_on_top_edge_of_window() {
        let a = win("0xa", 1, [12, 38], [941, 508], 0);
        let s = shown(place("0xa", &[a], &one_monitor(), BAR));
        assert_eq!(s.monitor, "DP-1");
        assert_eq!(
            s.rect,
            Rect {
                x: 12,
                y: 38,
                w: 941,
                h: BAR
            }
        );
        assert_eq!(
            s.margins,
            Margins {
                left: 12,
                top: 38,
                right: 1920 - 953,
                bottom: 1080 - 62
            }
        );
    }

    #[test]
    fn missing_window_is_gone() {
        assert_eq!(place("0xdead", &[], &one_monitor(), BAR), Placement::Gone);
    }

    #[test]
    fn other_workspace_hides() {
        let a = win("0xa", 2, [0, 0], [800, 600], 0);
        assert_eq!(
            hidden(place("0xa", &[a], &one_monitor(), BAR)),
            Hidden::OtherWorkspace
        );
    }

    #[test]
    fn unmapped_or_hidden_group_member_hides() {
        let mut a = win("0xa", 1, [0, 0], [800, 600], 0);
        a.hidden = true;
        assert_eq!(
            hidden(place("0xa", &[a.clone()], &one_monitor(), BAR)),
            Hidden::Unmapped
        );
        a.hidden = false;
        a.mapped = false;
        assert_eq!(
            hidden(place("0xa", &[a.clone()], &one_monitor(), BAR)),
            Hidden::Unmapped
        );
        // Hyprland 0.56 marks a background group tab `visible: false`.
        a.mapped = true;
        a.visible = false;
        assert_eq!(
            hidden(place("0xa", &[a], &one_monitor(), BAR)),
            Hidden::Unmapped
        );
    }

    #[test]
    fn special_workspace_over_window_hides() {
        let mut mons = one_monitor();
        mons[0].special_workspace = WorkspaceRef {
            id: -98,
            name: "special:sp".into(),
        };
        let a = win("0xa", 1, [0, 0], [800, 600], 1);
        assert_eq!(hidden(place("0xa", &[a], &mons, BAR)), Hidden::SpecialOpen);
    }

    #[test]
    fn window_on_open_special_workspace_shows() {
        let mut mons = one_monitor();
        mons[0].special_workspace = WorkspaceRef {
            id: -98,
            name: "special:sp".into(),
        };
        let a = win("0xa", -98, [100, 100], [800, 600], 0);
        let under = win("0xb", 1, [0, 0], [1920, 1080], 1);
        let s = shown(place("0xa", &[a, under], &mons, BAR));
        assert_eq!(s.rect.x, 100);
    }

    #[test]
    fn window_on_closed_special_workspace_hides() {
        let a = win("0xa", -98, [0, 0], [800, 600], 0);
        assert_eq!(
            hidden(place("0xa", &[a], &one_monitor(), BAR)),
            Hidden::OtherWorkspace
        );
    }

    #[test]
    fn special_workspace_on_other_monitor_does_not_hide() {
        let mut mons = vec![mon(0, "DP-1", 0, 1), mon(1, "DP-2", 1920, 2)];
        mons[1].special_workspace = WorkspaceRef {
            id: -98,
            name: "special:sp".into(),
        };
        let a = win("0xa", 1, [0, 0], [800, 600], 0);
        assert!(matches!(
            place("0xa", &[a], &mons, BAR),
            Placement::Shown(_)
        ));
    }

    #[test]
    fn pinned_window_shows_on_any_workspace_of_its_monitor() {
        let mut a = win("0xa", 7, [50, 50], [400, 300], 0);
        a.pinned = true;
        a.floating = true;
        let mons = one_monitor();
        assert!(matches!(
            place("0xa", &[a.clone()], &mons, BAR),
            Placement::Shown(_)
        ));
        let mut special = mons.clone();
        special[0].special_workspace = WorkspaceRef {
            id: -98,
            name: "special:sp".into(),
        };
        assert_eq!(
            hidden(place("0xa", &[a.clone()], &special, BAR)),
            Hidden::SpecialOpen
        );
        a.monitor = 5;
        assert_eq!(hidden(place("0xa", &[a], &mons, BAR)), Hidden::NoMonitor);
    }

    #[test]
    fn fullscreen_hides_but_maximized_shows() {
        let mut a = win("0xa", 1, [0, 0], [1920, 1080], 0);
        a.fullscreen = 2;
        assert_eq!(
            hidden(place("0xa", &[a.clone()], &one_monitor(), BAR)),
            Hidden::Fullscreen
        );
        a.fullscreen = 3;
        assert_eq!(
            hidden(place("0xa", &[a.clone()], &one_monitor(), BAR)),
            Hidden::Fullscreen
        );
        a.fullscreen = 1;
        let s = shown(place("0xa", &[a], &one_monitor(), BAR));
        assert_eq!(
            s.rect,
            Rect {
                x: 0,
                y: 0,
                w: 1920,
                h: BAR
            }
        );
    }

    #[test]
    fn another_fullscreen_window_covers() {
        let a = win("0xa", 1, [0, 0], [960, 1080], 1);
        let mut b = win("0xb", 1, [0, 0], [1920, 1080], 0);
        b.fullscreen = 2;
        assert_eq!(
            hidden(place("0xa", &[a.clone(), b.clone()], &one_monitor(), BAR)),
            Hidden::Covered
        );
        // Even when the band's window was focused more recently.
        let mut a = a;
        a.focus_history_id = 0;
        b.focus_history_id = 1;
        assert_eq!(
            hidden(place("0xa", &[a, b], &one_monitor(), BAR)),
            Hidden::Covered
        );
    }

    #[test]
    fn maximized_window_covers_tiled_neighbour() {
        let a = win("0xa", 1, [0, 0], [960, 1080], 0);
        let mut b = win("0xb", 1, [0, 0], [1920, 1080], 1);
        b.fullscreen = 1;
        assert_eq!(
            hidden(place("0xa", &[a, b], &one_monitor(), BAR)),
            Hidden::Covered
        );
    }

    #[test]
    fn floating_window_over_band_hides_it() {
        let a = win("0xa", 1, [0, 0], [960, 1080], 0);
        let mut f = win("0xf", 1, [100, 0], [800, 400], 1);
        f.floating = true;
        // Floating is above tiled even though the tiled window has focus.
        assert_eq!(
            hidden(place("0xa", &[a, f], &one_monitor(), BAR)),
            Hidden::Covered
        );
    }

    #[test]
    fn floating_window_below_band_does_not_hide() {
        let a = win("0xa", 1, [0, 0], [960, 1080], 0);
        let mut f = win("0xf", 1, [100, BAR], [800, 400], 1);
        f.floating = true;
        assert!(matches!(
            place("0xa", &[a, f], &one_monitor(), BAR),
            Placement::Shown(_)
        ));
    }

    #[test]
    fn partly_covered_band_shrinks_to_widest_gap() {
        let a = win("0xa", 1, [0, 0], [1000, 800], 0);
        let mut f = win("0xf", 1, [800, 0], [300, 300], 1);
        f.floating = true;
        let s = shown(place("0xa", &[a, f], &one_monitor(), BAR));
        assert_eq!(
            s.rect,
            Rect {
                x: 0,
                y: 0,
                w: 800,
                h: BAR
            }
        );
        assert_eq!(s.margins.right, 1920 - 800);
    }

    #[test]
    fn covered_in_the_middle_keeps_the_wider_side() {
        let a = win("0xa", 1, [0, 0], [1000, 800], 0);
        let mut f = win("0xf", 1, [300, 0], [100, 100], 1);
        f.floating = true;
        let s = shown(place("0xa", &[a, f], &one_monitor(), BAR));
        assert_eq!((s.rect.x, s.rect.w), (400, 600));
    }

    #[test]
    fn floating_stack_uses_focus_order() {
        let mut a = win("0xa", 1, [0, 0], [600, 400], 1);
        let mut b = win("0xb", 1, [50, 0], [600, 400], 0);
        a.floating = true;
        b.floating = true;
        assert_eq!(
            hidden(place("0xa", &[a.clone(), b.clone()], &one_monitor(), BAR)),
            Hidden::Covered
        );
        // Focus a: it rises above b.
        a.focus_history_id = 0;
        b.focus_history_id = 1;
        assert!(matches!(
            place("0xa", &[a, b], &one_monitor(), BAR),
            Placement::Shown(_)
        ));
    }

    #[test]
    fn floating_band_not_hidden_by_tiled_window_behind() {
        let mut a = win("0xa", 1, [100, 100], [600, 400], 1);
        a.floating = true;
        let t = win("0xt", 1, [0, 0], [1920, 1080], 0);
        assert!(matches!(
            place("0xa", &[a, t], &one_monitor(), BAR),
            Placement::Shown(_)
        ));
    }

    #[test]
    fn pinned_floats_above_ordinary_floats() {
        let mut a = win("0xa", 1, [0, 0], [600, 400], 1);
        a.floating = true;
        a.pinned = true;
        let mut b = win("0xb", 1, [0, 0], [600, 400], 0);
        b.floating = true;
        assert!(matches!(
            place("0xa", &[a, b], &one_monitor(), BAR),
            Placement::Shown(_)
        ));
    }

    #[test]
    fn windows_on_other_workspaces_do_not_cover() {
        let a = win("0xa", 1, [0, 0], [960, 1080], 1);
        let mut f = win("0xf", 2, [0, 0], [960, 1080], 0);
        f.floating = true;
        let mut g = win("0xg", 1, [0, 0], [960, 1080], 0);
        g.floating = true;
        g.hidden = true;
        assert!(matches!(
            place("0xa", &[a, f, g], &one_monitor(), BAR),
            Placement::Shown(_)
        ));
    }

    #[test]
    fn special_workspace_windows_cover_nothing_they_are_not_over() {
        // A window on the open special workspace sits over the normal one,
        // but the normal window's band is hidden anyway (SpecialOpen).
        let mut mons = one_monitor();
        mons[0].special_workspace = WorkspaceRef {
            id: -98,
            name: "special:sp".into(),
        };
        let s = win("0xs", -98, [0, 0], [600, 400], 1);
        let mut f = win("0xf", -98, [0, 0], [600, 400], 0);
        f.floating = true;
        // Floating special window over the banded special window.
        assert_eq!(hidden(place("0xs", &[s, f], &mons, BAR)), Hidden::Covered);
    }

    #[test]
    fn second_monitor_offsets_margins() {
        let mons = vec![mon(0, "DP-1", 0, 1), mon(1, "DP-2", 1920, 2)];
        let mut a = win("0xa", 2, [1930, 40], [900, 500], 0);
        a.monitor = 1;
        let s = shown(place("0xa", &[a], &mons, BAR));
        assert_eq!(s.monitor, "DP-2");
        assert_eq!(
            s.rect,
            Rect {
                x: 10,
                y: 40,
                w: 900,
                h: BAR
            }
        );
        assert_eq!(
            s.global,
            Rect {
                x: 1930,
                y: 40,
                w: 900,
                h: BAR
            }
        );
        assert_eq!(s.margins.right, 1920 - 910);
    }

    #[test]
    fn workspace_moved_to_other_monitor_follows() {
        // Visibility comes from which monitor shows the workspace, even if
        // the client's monitor field lags behind.
        let mons = vec![mon(0, "DP-1", 0, 1), mon(1, "DP-2", 1920, 3)];
        let mut a = win("0xa", 3, [1930, 40], [900, 500], 0);
        a.monitor = 0;
        assert_eq!(shown(place("0xa", &[a], &mons, BAR)).monitor, "DP-2");
    }

    #[test]
    fn scaled_monitor_uses_logical_size() {
        let mut mons = one_monitor();
        mons[0].width = 2880;
        mons[0].height = 1620;
        mons[0].scale = 1.5;
        let a = win("0xa", 1, [960, 0], [960, 1080], 0);
        let s = shown(place("0xa", &[a], &mons, BAR));
        assert_eq!(s.margins.right, 0);
        assert_eq!(s.margins.bottom, 1080 - BAR);
    }

    #[test]
    fn rotated_monitor_swaps_axes() {
        let mut mons = one_monitor();
        mons[0].transform = 1;
        let a = win("0xa", 1, [0, 1000], [1080, 900], 0);
        let s = shown(place("0xa", &[a], &mons, BAR));
        assert_eq!(s.margins.right, 0);
        assert_eq!(s.margins.bottom, 1920 - 1000 - BAR);
    }

    #[test]
    fn window_hanging_off_the_left_is_clipped() {
        let a = win("0xa", 1, [-200, 100], [600, 400], 0);
        let s = shown(place("0xa", &[a], &one_monitor(), BAR));
        assert_eq!(
            s.rect,
            Rect {
                x: 0,
                y: 100,
                w: 400,
                h: BAR
            }
        );
        assert!(s.margins.left >= 0 && s.margins.right >= 0);
    }

    #[test]
    fn window_hanging_off_the_right_is_clipped() {
        let a = win("0xa", 1, [1700, 100], [600, 400], 0);
        let s = shown(place("0xa", &[a], &one_monitor(), BAR));
        assert_eq!(
            s.rect,
            Rect {
                x: 1700,
                y: 100,
                w: 220,
                h: BAR
            }
        );
        assert_eq!(s.margins.right, 0);
    }

    #[test]
    fn window_above_the_top_slides_band_down() {
        let a = win("0xa", 1, [1, -21], [1038, 1050], 0);
        let s = shown(place("0xa", &[a], &one_monitor(), BAR));
        assert_eq!(s.rect.y, 0);
        assert_eq!(s.margins.top, 0);
    }

    #[test]
    fn window_mostly_below_the_bottom() {
        let a = win("0xa", 1, [0, 1070], [600, 400], 0);
        assert_eq!(
            hidden(place("0xa", &[a], &one_monitor(), BAR)),
            Hidden::TooSmall
        );
        let b = win("0xb", 1, [0, 1080], [600, 400], 0);
        assert_eq!(
            hidden(place("0xb", &[b], &one_monitor(), BAR)),
            Hidden::OffScreen
        );
    }

    #[test]
    fn window_entirely_off_screen() {
        for at in [[-700, 0], [1920, 0], [0, -500], [0, 1200]] {
            let a = win("0xa", 1, at, [600, 400], 0);
            assert_eq!(
                hidden(place("0xa", &[a], &one_monitor(), BAR)),
                Hidden::OffScreen,
                "{at:?}"
            );
        }
    }

    #[test]
    fn tiny_windows() {
        let short = win("0xa", 1, [0, 0], [600, BAR - 1], 0);
        assert_eq!(
            hidden(place("0xa", &[short], &one_monitor(), BAR)),
            Hidden::TooSmall
        );
        let narrow = win("0xa", 1, [0, 0], [MIN_WIDTH - 1, 400], 0);
        assert_eq!(
            hidden(place("0xa", &[narrow], &one_monitor(), BAR)),
            Hidden::TooSmall
        );
        let exact = win("0xa", 1, [0, 0], [MIN_WIDTH, BAR], 0);
        assert!(matches!(
            place("0xa", &[exact], &one_monitor(), BAR),
            Placement::Shown(_)
        ));
    }

    #[test]
    fn disabled_monitor_is_ignored() {
        let mut mons = one_monitor();
        mons[0].disabled = true;
        let a = win("0xa", 1, [0, 0], [600, 400], 0);
        assert_eq!(
            hidden(place("0xa", &[a], &mons, BAR)),
            Hidden::OtherWorkspace
        );
    }

    #[test]
    fn no_monitors_at_all() {
        let a = win("0xa", 1, [0, 0], [600, 400], 0);
        assert_eq!(hidden(place("0xa", &[a], &[], BAR)), Hidden::OtherWorkspace);
    }

    #[test]
    fn margins_always_add_up() {
        let mons = vec![mon(0, "DP-1", 0, 1), mon(1, "DP-2", 1920, 2)];
        for x in (-400..2400).step_by(97) {
            for y in (-300..1200).step_by(83) {
                for (ws, m) in [(1, 0), (2, 1)] {
                    let mut a = win("0xa", ws, [x + m * 1920, y], [500, 350], 0);
                    a.monitor = m.into();
                    if let Placement::Shown(s) = place("0xa", &[a], &mons, BAR) {
                        let mg = s.margins;
                        assert!(
                            mg.left >= 0 && mg.top >= 0 && mg.right >= 0 && mg.bottom >= 0,
                            "{mg:?}"
                        );
                        assert_eq!(mg.left + s.rect.w + mg.right, 1920);
                        assert_eq!(mg.top + s.rect.h + mg.bottom, 1080);
                        assert!(s.rect.w >= MIN_WIDTH);
                    }
                }
            }
        }
    }

    #[test]
    fn widest_gap_cases() {
        assert_eq!(widest_gap(0, 100, &mut []), (0, 100));
        assert_eq!(widest_gap(0, 100, &mut [(0, 100)]), (0, 0));
        assert_eq!(widest_gap(0, 100, &mut [(-50, 150)]), (0, 0));
        assert_eq!(widest_gap(0, 100, &mut [(10, 20)]), (20, 100));
        assert_eq!(widest_gap(0, 100, &mut [(80, 90)]), (0, 80));
        assert_eq!(widest_gap(0, 100, &mut [(60, 70), (10, 20)]), (20, 60));
        assert_eq!(widest_gap(0, 100, &mut [(10, 50), (20, 30)]), (50, 100));
        assert_eq!(widest_gap(0, 100, &mut [(200, 300), (-50, -10)]), (0, 100));
        assert_eq!(widest_gap(10, 110, &mut [(0, 10)]), (10, 110));
    }
}
