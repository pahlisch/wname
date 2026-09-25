//! End-to-end tests against a real, throwaway Hyprland.
//!
//! These close every window on the instance they run against, so they only
//! run when WNAME_TEST_NESTED=1 and the instance was started with
//! tests/hyprland-test.conf. Use scripts/test-nested.sh, which sets that up.
//!
//! They check the band through `hyprctl layers`, which reports where the
//! compositor actually put each layer surface.

#[path = "support/pointer.rs"]
mod pointer;

use serde_json::Value;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

const NAMESPACE: &str = "wname";
const WAIT: Duration = Duration::from_secs(3);

struct Env {
    sig: String,
    wayland: String,
}

fn env() -> &'static Env {
    static ENV: OnceLock<Env> = OnceLock::new();
    ENV.get_or_init(|| {
        assert_eq!(
            std::env::var("WNAME_TEST_NESTED").as_deref(),
            Ok("1"),
            "run these through scripts/test-nested.sh"
        );
        let sig =
            std::env::var("HYPRLAND_INSTANCE_SIGNATURE").expect("HYPRLAND_INSTANCE_SIGNATURE");
        let wayland = std::env::var("WAYLAND_DISPLAY").expect("WAYLAND_DISPLAY");
        let instances: Value =
            serde_json::from_str(&run("hyprctl", &["instances", "-j"])).expect("instances json");
        let pid = instances
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["instance"] == sig.as_str())
            .and_then(|i| i["pid"].as_u64())
            .expect("instance is running");
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        assert!(
            String::from_utf8_lossy(&cmdline).contains("hyprland-test.conf"),
            "refusing to run: instance {sig} was not started with tests/hyprland-test.conf"
        );
        Env { sig, wayland }
    })
}

/// The tests share one compositor, so they take turns.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn run(cmd: &str, args: &[&str]) -> String {
    let out = Command::new(cmd).args(args).output().expect(cmd);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn hyprctl(args: &[&str]) -> String {
    let e = env();
    let mut full = vec!["-i", e.sig.as_str()];
    full.extend_from_slice(args);
    run("hyprctl", &full)
}

fn hjson(what: &str) -> Value {
    let raw = hyprctl(&["-j", what]);
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{what}: {e}: {raw}"))
}

fn dispatch(args: &str) {
    let parts: Vec<&str> = std::iter::once("dispatch").chain(args.split(' ')).collect();
    let out = hyprctl(&parts);
    assert!(out.trim() == "ok", "dispatch {args}: {out}");
}

fn wname(args: &[&str]) -> Output {
    let e = env();
    Command::new(env!("CARGO_BIN_EXE_wname"))
        .args(args)
        .env("HYPRLAND_INSTANCE_SIGNATURE", &e.sig)
        .env("WAYLAND_DISPLAY", &e.wayland)
        .output()
        .expect("wname")
}

fn wname_ok(args: &[&str]) -> String {
    let out = wname(args);
    assert!(
        out.status.success(),
        "wname {args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Geo {
    x: i64,
    y: i64,
    w: i64,
    h: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Layer {
    monitor: String,
    geo: Geo,
}

/// Every mapped band surface.
fn bands() -> Vec<Layer> {
    let mut out = Vec::new();
    let layers = hjson("layers");
    for (monitor, v) in layers.as_object().unwrap() {
        for (_, list) in v["levels"].as_object().unwrap() {
            for l in list.as_array().unwrap() {
                if l["namespace"] == NAMESPACE {
                    out.push(Layer {
                        monitor: monitor.clone(),
                        geo: Geo {
                            x: l["x"].as_i64().unwrap(),
                            y: l["y"].as_i64().unwrap(),
                            w: l["w"].as_i64().unwrap(),
                            h: l["h"].as_i64().unwrap(),
                        },
                    });
                }
            }
        }
    }
    out
}

fn client(address: &str) -> Value {
    hjson("clients")
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["address"] == address)
        .cloned()
        .unwrap_or_else(|| panic!("no client {address}"))
}

fn monitor_named(name: &str) -> Value {
    hjson("monitors")
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("no monitor {name}"))
}

/// Where the band for this window should be, in global layout coordinates,
/// when nothing covers it and it is fully on screen.
fn expected(address: &str) -> Geo {
    let c = client(address);
    Geo {
        x: c["at"][0].as_i64().unwrap(),
        y: c["at"][1].as_i64().unwrap(),
        w: c["size"][0].as_i64().unwrap(),
        h: 0,
    }
}

/// Wait until exactly one band is up and it sits on the window's top edge.
fn assert_band_on(address: &str) -> Layer {
    wait_for(&format!("band on {address}"), || {
        let want = expected(address);
        let got = bands();
        (got.len() == 1 && {
            let g = got[0].geo;
            g.x == want.x && g.y == want.y && g.w == want.w && g.h > 10
        })
        .then(|| got[0].clone())
    })
}

fn assert_no_bands() {
    wait_for("no bands on screen", || bands().is_empty().then_some(()));
}

fn active_address() -> String {
    hjson("activewindow")["address"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

struct Window {
    address: String,
    child: Child,
}

impl Drop for Window {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn open(tag: &str) -> Window {
    open_running(tag, &["sleep", "600"])
}

/// Open a foot window running `command`.
fn open_running(tag: &str, command: &[&str]) -> Window {
    let e = env();
    let app_id = format!("wname-test-{tag}");
    let child = Command::new("foot")
        .args([&format!("--app-id={app_id}"), "-T", tag])
        .args(command)
        .env("WAYLAND_DISPLAY", &e.wayland)
        .env("HYPRLAND_INSTANCE_SIGNATURE", &e.sig)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("foot");
    let address = wait_for(&format!("window {tag}"), || {
        hjson("clients")
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["class"] == app_id.as_str() && c["mapped"] == true)
            .map(|c| c["address"].as_str().unwrap().to_string())
    });
    wait_for("focus on new window", || {
        (active_address() == address).then_some(())
    });
    Window { address, child }
}

/// Daemons serving the nested instance under test. Never matches a daemon
/// on your real session, even one started from the same binary.
fn test_daemons() -> Vec<u32> {
    let want = format!("HYPRLAND_INSTANCE_SIGNATURE={}", env().sig);
    let mut pids = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let args: Vec<&[u8]> = cmdline.split(|&b| b == 0).collect();
        if !args.iter().any(|a| *a == b"--daemon") {
            continue;
        }
        let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
        if environ.split(|&b| b == 0).any(|v| v == want.as_bytes()) {
            pids.push(pid);
        }
    }
    pids
}

fn daemon_running() -> bool {
    let e = env();
    let path = format!(
        "{}/wname/{}/ctl.sock",
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()),
        e.sig
    );
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// Start each test from an empty workspace 1 on a single monitor.
fn reset() -> MutexGuard<'static, ()> {
    let guard = serial();
    let _ = wname(&["--quit"]);
    wait_for("daemon to stop", || (!daemon_running()).then_some(()));
    for c in hjson("clients").as_array().unwrap() {
        let addr = c["address"].as_str().unwrap();
        hyprctl(&["dispatch", "closewindow", &format!("address:{addr}")]);
    }
    for m in hjson("monitors").as_array().unwrap() {
        let name = m["name"].as_str().unwrap();
        if name.starts_with("HEADLESS") {
            hyprctl(&["output", "remove", name]);
        }
        let special = m["specialWorkspace"]["name"].as_str().unwrap_or("");
        if !special.is_empty() {
            let short = special.trim_start_matches("special:");
            hyprctl(&["dispatch", "togglespecialworkspace", short]);
        }
    }
    wait_for("no windows", || {
        hjson("clients")
            .as_array()
            .unwrap()
            .is_empty()
            .then_some(())
    });
    hyprctl(&["keyword", "animations:enabled", "0"]);
    dispatch("workspace 1");
    guard
}

fn name(win: &Window, title: &str, extra: &[&str]) -> String {
    let mut args = vec!["-n", title, "-w", win.address.as_str()];
    args.extend_from_slice(extra);
    wname_ok(&args)
}

/// Make a window floating at an exact position and size.
fn float(win: &Window, x: i64, y: i64, w: i64, h: i64) {
    if client(&win.address)["floating"] != true {
        dispatch(&format!("setfloating address:{}", win.address));
    }
    dispatch(&format!(
        "resizewindowpixel exact {w} {h},address:{}",
        win.address
    ));
    dispatch(&format!(
        "movewindowpixel exact {x} {y},address:{}",
        win.address
    ));
    wait_for("window geometry", || {
        let c = client(&win.address);
        (c["at"][0] == x && c["at"][1] == y && c["size"][0] == w && c["size"][1] == h).then_some(())
    });
}

// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn band_appears_on_focused_window() {
    let _g = reset();
    let a = open("a");
    let out = wname_ok(&["-n", "Blog project", "-c", "red"]);
    assert!(
        out.contains("“Blog project”") && out.contains("in red"),
        "{out}"
    );
    assert_band_on(&a.address);
    let list = wname_ok(&["--list"]);
    assert!(
        list.contains(&a.address) && list.contains("Blog project") && list.contains("red"),
        "{list}"
    );
    assert!(
        !list.contains("hidden"),
        "listed as hidden right after naming: {list}"
    );
}

#[test]
#[ignore]
fn rename_keeps_color_and_color_can_change() {
    let _g = reset();
    let a = open("a");
    name(&a, "One", &["-c", "teal"]);
    let out = name(&a, "Two", &[]);
    assert!(out.contains("teal, kept"), "{out}");
    let out = name(&a, "Three", &["-c", "#123"]);
    assert!(out.contains("#112233"), "{out}");
    let list = wname_ok(&["--list"]);
    assert!(list.contains("Three") && !list.contains("Two"), "{list}");
    assert_band_on(&a.address);
}

#[test]
#[ignore]
fn random_colors_differ_between_bands() {
    let _g = reset();
    let a = open("a");
    let b = open("b");
    name(&a, "A", &[]);
    name(&b, "B", &[]);
    let list = wname_ok(&["--list"]);
    let colors: Vec<&str> = list
        .lines()
        .map(|l| l.split_whitespace().nth(2).unwrap())
        .collect();
    assert_eq!(colors.len(), 2);
    assert_ne!(colors[0], colors[1], "{list}");
}

#[test]
#[ignore]
fn workspace_switch_hides_and_restores() {
    let _g = reset();
    let a = open("a");
    name(&a, "A", &[]);
    assert_band_on(&a.address);
    dispatch("workspace 2");
    assert_no_bands();
    dispatch("workspace 1");
    assert_band_on(&a.address);
}

#[test]
#[ignore]
fn hides_immediately_on_workspace_switch() {
    let _g = reset();
    let a = open("a");
    name(&a, "A", &[]);
    assert_band_on(&a.address);
    // Well under one slow poll: the event, not the timer, must do this.
    for _ in 0..5 {
        dispatch("workspace 2");
        let t = Instant::now();
        assert_no_bands();
        assert!(
            t.elapsed() < Duration::from_millis(400),
            "took {:?}",
            t.elapsed()
        );
        dispatch("workspace 1");
        assert_band_on(&a.address);
    }
}

#[test]
#[ignore]
fn window_moved_to_other_workspace() {
    let _g = reset();
    let a = open("a");
    name(&a, "A", &[]);
    dispatch(&format!("movetoworkspacesilent 3,address:{}", a.address));
    assert_no_bands();
    dispatch("workspace 3");
    assert_band_on(&a.address);
    dispatch("workspace 1");
    assert_no_bands();
}

#[test]
#[ignore]
fn band_follows_retiling() {
    let _g = reset();
    let a = open("a");
    name(&a, "A", &[]);
    let full = assert_band_on(&a.address);
    let b = open("b");
    // Opening b splits the workspace, so a's band must follow a's new
    // geometry, whichever way the split went.
    let half = assert_band_on(&a.address);
    assert_ne!(half.geo, full.geo);
    drop(b);
    wait_for("a back to full size", || {
        let g = bands();
        (g.len() == 1 && g[0].geo == full.geo).then_some(())
    });
}

#[test]
#[ignore]
fn special_workspace() {
    let _g = reset();
    let a = open("a");
    name(&a, "A", &[]);
    assert_band_on(&a.address);
    dispatch("togglespecialworkspace scratch");
    assert_no_bands();
    dispatch("togglespecialworkspace scratch");
    assert_band_on(&a.address);

    dispatch(&format!(
        "movetoworkspacesilent special:scratch,address:{}",
        a.address
    ));
    assert_no_bands();
    dispatch("togglespecialworkspace scratch");
    assert_band_on(&a.address);
    dispatch("togglespecialworkspace scratch");
    assert_no_bands();
}

#[test]
#[ignore]
fn floating_window_over_band() {
    let _g = reset();
    let a = open("a");
    name(&a, "A", &[]);
    let full = assert_band_on(&a.address);
    let a_geo = expected(&a.address);
    let f = open("f");
    // Cover the band completely.
    float(&f, a_geo.x - 20, a_geo.y, a_geo.w + 40, 300);
    assert_no_bands();
    // Cover only the right third: the band shrinks to the uncovered part.
    float(
        &f,
        a_geo.x + a_geo.w - a_geo.w / 3,
        a_geo.y,
        a_geo.w / 3,
        300,
    );
    let part = wait_for("partial band", || {
        let g = bands();
        (g.len() == 1 && g[0].geo.x == full.geo.x && g[0].geo.w < full.geo.w).then(|| g[0].clone())
    });
    let f_x = client(&f.address)["at"][0].as_i64().unwrap();
    assert_eq!(
        part.geo.x + part.geo.w,
        f_x,
        "band ends where the float starts"
    );
    // Move it below the band: full band again.
    float(&f, a_geo.x, a_geo.y + 200, a_geo.w / 3, 300);
    assert_band_on(&a.address);
}

#[test]
#[ignore]
fn band_on_floating_window_follows_moves_and_resizes() {
    let _g = reset();
    let a = open("a");
    float(&a, 200, 200, 400, 300);
    name(&a, "A", &[]);
    assert_band_on(&a.address);
    for (x, y, w) in [(100, 100, 500), (300, 250, 700), (50, 400, 320)] {
        float(&a, x, y, w, 300);
        let t = Instant::now();
        let got = assert_band_on(&a.address);
        assert_eq!(got.geo.w, w);
        assert!(
            t.elapsed() < Duration::from_millis(600),
            "slow to follow: {:?}",
            t.elapsed()
        );
    }
}

#[test]
#[ignore]
fn fullscreen_hides_maximize_keeps() {
    let _g = reset();
    let a = open("a");
    name(&a, "A", &[]);
    assert_band_on(&a.address);
    dispatch("fullscreen 0");
    assert_no_bands();
    dispatch("fullscreen 0");
    assert_band_on(&a.address);
    dispatch("fullscreen 1");
    let max = assert_band_on(&a.address);
    let mon = monitor_named(&max.monitor);
    assert!(max.geo.w > mon["width"].as_i64().unwrap() / 2);
    dispatch("fullscreen 1");
    assert_band_on(&a.address);
}

#[test]
#[ignore]
fn other_window_fullscreen_covers_band() {
    let _g = reset();
    let a = open("a");
    let b = open("b");
    name(&a, "A", &[]);
    assert_band_on(&a.address);
    dispatch(&format!("focuswindow address:{}", b.address));
    dispatch("fullscreen 0");
    assert_no_bands();
    dispatch("fullscreen 0");
    assert_band_on(&a.address);
}

#[test]
#[ignore]
fn pinned_window_keeps_band_across_workspaces() {
    let _g = reset();
    let a = open("a");
    float(&a, 100, 100, 600, 400);
    dispatch(&format!("pin address:{}", a.address));
    name(&a, "A", &[]);
    assert_band_on(&a.address);
    dispatch("workspace 4");
    assert_band_on(&a.address);
    dispatch("togglespecialworkspace scratch");
    assert_no_bands();
    dispatch("togglespecialworkspace scratch");
    assert_band_on(&a.address);
    dispatch(&format!("pin address:{}", a.address));
    dispatch("workspace 5");
    assert_no_bands();
}

#[test]
#[ignore]
fn grouped_windows_show_only_active_tab() {
    let _g = reset();
    let a = open("a");
    dispatch("togglegroup");
    let b = open("b");
    // With a group active, a new window joins it as the visible tab.
    wait_for("b in a's group", || {
        client(&b.address)["grouped"]
            .as_array()
            .is_some_and(|g| g.len() == 2)
            .then_some(())
    });
    name(&a, "A", &[]);
    assert_no_bands();
    dispatch("changegroupactive f");
    wait_for("a active in group", || {
        (client(&a.address)["visible"] == true).then_some(())
    });
    assert_band_on(&a.address);
}

#[test]
#[ignore]
fn closing_window_removes_band_and_daemon_exits() {
    let _g = reset();
    let a = open("a");
    let b = open("b");
    name(&a, "A", &[]);
    name(&b, "B", &[]);
    drop(a);
    let list = wait_for("a's band gone from list", || {
        let l = wname_ok(&["--list"]);
        (l.lines().count() == 1).then_some(l)
    });
    assert!(list.contains("B"), "{list}");
    drop(b);
    assert_no_bands();
    // Nothing left to draw: the daemon leaves on its own.
    let deadline = Instant::now() + Duration::from_secs(8);
    while daemon_running() {
        assert!(
            Instant::now() < deadline,
            "daemon still running with no bands"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore]
fn clear_and_clear_all() {
    let _g = reset();
    let a = open("a");
    let b = open("b");
    name(&a, "A", &[]);
    name(&b, "B", &[]);
    let out = wname_ok(&["--clear", "-w", &a.address]);
    assert!(out.contains("Cleared"), "{out}");
    assert_band_on(&b.address);
    let out = wname_ok(&["--clear", "-w", &a.address]);
    assert!(out.contains("No band"), "{out}");
    name(&a, "A", &[]);
    let out = wname_ok(&["--clear-all"]);
    assert!(out.contains("Cleared 2 bands"), "{out}");
    assert_no_bands();
    let out = wname_ok(&["--list"]);
    assert!(out.contains("No bands"), "{out}");
}

#[test]
#[ignore]
fn clear_on_focused_window() {
    let _g = reset();
    let a = open("a");
    wname_ok(&["-n", "A"]);
    assert_band_on(&a.address);
    wname_ok(&["--clear"]);
    assert_no_bands();
}

#[test]
#[ignore]
fn second_monitor() {
    let _g = reset();
    let main = hjson("monitors")[0]["name"].as_str().unwrap().to_string();
    hyprctl(&["output", "create", "headless"]);
    let second = wait_for("headless monitor", || {
        hjson("monitors")
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"].as_str().unwrap().starts_with("HEADLESS"))
            .map(|m| m["name"].as_str().unwrap().to_string())
    });
    // A new output takes focus; open the window on the original one.
    dispatch(&format!("focusmonitor {main}"));
    let a = open("a");
    name(&a, "A", &[]);
    let first = assert_band_on(&a.address);
    assert_eq!(first.monitor, main);

    dispatch(&format!("focuswindow address:{}", a.address));
    dispatch(&format!("movewindow mon:{second}"));
    let moved = assert_band_on(&a.address);
    assert_eq!(moved.monitor, second);

    // Switching workspaces on the other monitor does not touch this band.
    dispatch(&format!("focusmonitor {}", first.monitor));
    dispatch("workspace 7");
    assert_band_on(&a.address);

    // Unplug: the window comes back to the first monitor, and so does the band.
    hyprctl(&["output", "remove", &second]);
    wait_for("band back on first monitor", || {
        let g = bands();
        let c = client(&a.address);
        let ws = c["workspace"]["id"].as_i64().unwrap();
        let shown = monitor_named(&first.monitor)["activeWorkspace"]["id"]
            .as_i64()
            .unwrap();
        if ws != shown {
            dispatch(&format!("workspace {ws}"));
        }
        (g.len() == 1 && g[0].monitor == first.monitor).then_some(())
    });
    assert_band_on(&a.address);
}

#[test]
#[ignore]
fn clicks_pass_through_the_band() {
    let _g = reset();
    let a = open("a");
    let b = open("b");
    name(&b, "B", &[]);
    let band = assert_band_on(&b.address);
    dispatch(&format!("focuswindow address:{}", a.address));
    assert_eq!(active_address(), a.address);
    // Glide from the middle of a onto the band over b. With follow_mouse,
    // focus moves to b only if the pointer reaches b, that is, only if the
    // band does not take the pointer.
    let ag = expected(&a.address);
    let mon = &hjson("monitors")[0];
    let extent = (
        mon["width"].as_u64().unwrap() as u32,
        mon["height"].as_u64().unwrap() as u32,
    );
    let from = ((ag.x + ag.w / 2) as u32, 300);
    let to = (
        (band.geo.x + band.geo.w / 2) as u32,
        (band.geo.y + band.geo.h / 2) as u32,
    );
    pointer::move_to(from, to, extent);
    wait_for("pointer to reach the window under the band", || {
        (active_address() == b.address).then_some(())
    });
}

#[test]
#[ignore]
fn daemon_restart_restores_bands() {
    let _g = reset();
    let a = open("a");
    let b = open("b");
    name(&a, "A", &["-c", "blue"]);
    name(&b, "B", &["-c", "green"]);
    assert_eq!(
        wait_for("two bands", || {
            let g = bands();
            (g.len() == 2).then_some(g)
        })
        .len(),
        2
    );
    let pids = test_daemons();
    assert_eq!(pids.len(), 1, "exactly one daemon: {pids:?}");
    run("kill", &[&pids[0].to_string()]);
    wait_for("daemon to stop", || (!daemon_running()).then_some(()));
    assert_no_bands();
    // Any request that needs the daemon brings it back with both bands.
    let c = open("c");
    name(&c, "C", &[]);
    let list = wait_for("three bands listed", || {
        let l = wname_ok(&["--list"]);
        (l.lines().count() == 3).then_some(l)
    });
    assert!(list.contains("blue") && list.contains("green"), "{list}");
}

#[test]
#[ignore]
fn concurrent_first_calls_start_one_daemon() {
    let _g = reset();
    let wins: Vec<Window> = (0..4).map(|i| open(&format!("w{i}"))).collect();
    let handles: Vec<_> = wins
        .iter()
        .map(|w| {
            let addr = w.address.clone();
            std::thread::spawn(move || wname(&["-n", "X", "-w", &addr]))
        })
        .collect();
    for h in handles {
        let out = h.join().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let list = wname_ok(&["--list"]);
    assert_eq!(list.lines().count(), 4, "{list}");
    let pids = test_daemons();
    assert_eq!(pids.len(), 1, "{pids:?}");
}

#[test]
#[ignore]
fn user_errors() {
    let _g = reset();
    let _a = open("a");
    let out = wname(&["-n", "X", "-w", "0xdeadbeef"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no window"));
    let out = wname(&["-n", "X", "-c", "chartreuse"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown color"));
    let out = wname(&["-n", "   "]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("empty"));
    let out = wname(&[]);
    assert!(!out.status.success());
    assert_no_bands();
}

#[test]
#[ignore]
fn no_focused_window() {
    let _g = reset();
    let out = wname(&["-n", "X"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no focused window"));
}

#[test]
#[ignore]
fn long_titles_do_not_widen_the_band() {
    let _g = reset();
    let a = open("a");
    float(&a, 100, 100, 200, 200);
    let long = "A very long name that is much wider than two hundred pixels for sure";
    name(&a, long, &[]);
    let band = assert_band_on(&a.address);
    assert_eq!(band.geo.w, 200);
}

#[test]
#[ignore]
fn band_is_hidden_on_tiny_windows_and_returns() {
    let _g = reset();
    let a = open("a");
    float(&a, 100, 100, 400, 300);
    name(&a, "A", &[]);
    assert_band_on(&a.address);
    // Push it mostly off the right edge of the monitor.
    let mon = &hjson("monitors")[0];
    let right = mon["x"].as_i64().unwrap() + mon["width"].as_i64().unwrap();
    dispatch(&format!(
        "movewindowpixel exact {} 100,address:{}",
        right - 10,
        a.address
    ));
    assert_no_bands();
    dispatch(&format!(
        "movewindowpixel exact {} 100,address:{}",
        right - 200,
        a.address
    ));
    let clipped = wait_for("clipped band", || {
        let g = bands();
        (g.len() == 1).then(|| g[0].clone())
    });
    assert_eq!(clipped.geo.x + clipped.geo.w, right);
}

#[test]
#[ignore]
fn animated_retile_waits_for_the_window() {
    let _g = reset();
    hyprctl(&["keyword", "animations:enabled", "1"]);
    // Window moves take 600 ms; everything else is quick.
    hyprctl(&["keyword", "animation", "global,1,1,default"]);
    hyprctl(&["keyword", "animation", "windowsMove,1,6,default"]);
    let a = open("a");
    name(&a, "A", &[]);
    let before = assert_band_on(&a.address);
    let t = Instant::now();
    let _b = open("b");
    // The band leaves at once instead of jumping ahead of the window...
    wait_for("band to step aside", || bands().is_empty().then_some(()));
    // ...and comes back on the window once the move is done.
    let after = assert_band_on(&a.address);
    let took = t.elapsed();
    assert_ne!(after.geo, before.geo);
    assert!(
        took >= Duration::from_millis(500),
        "came back after {took:?}, before the animation ended"
    );
    assert!(took < Duration::from_secs(3), "came back after {took:?}");
    hyprctl(&["keyword", "animations:enabled", "0"]);
}

/// Add a headless output with the given monitor rule tail, e.g. "1.5" or
/// "1,transform,1", and return its name. The main output keeps focus.
fn add_output(rule: &str) -> (String, String) {
    let main = hjson("monitors")[0]["name"].as_str().unwrap().to_string();
    hyprctl(&["output", "create", "headless"]);
    let name = wait_for("headless monitor", || {
        hjson("monitors")
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"].as_str().unwrap().starts_with("HEADLESS"))
            .map(|m| m["name"].as_str().unwrap().to_string())
    });
    hyprctl(&[
        "keyword",
        "monitor",
        &format!("{name},1920x1080,auto,{rule}"),
    ]);
    dispatch(&format!("focusmonitor {main}"));
    (main, name)
}

fn check_band_on_output(rule: &str, want_scale: f64, want_transform: i64) {
    let _g = reset();
    let (_main, out) = add_output(rule);
    wait_for("monitor rule applied", || {
        let m = monitor_named(&out);
        ((m["scale"].as_f64().unwrap() - want_scale).abs() < 0.01
            && m["transform"] == want_transform)
            .then_some(())
    });
    let a = open("a");
    name(&a, "A", &[]);
    assert_band_on(&a.address);
    dispatch(&format!("focuswindow address:{}", a.address));
    dispatch(&format!("movewindow mon:{out}"));
    let band = assert_band_on(&a.address);
    assert_eq!(band.monitor, out);
    // The band lies inside the monitor's logical area.
    let m = monitor_named(&out);
    let (w, h) = (m["width"].as_f64().unwrap(), m["height"].as_f64().unwrap());
    let (lw, lh) = if want_transform % 2 == 1 {
        (h, w)
    } else {
        (w, h)
    };
    let (lw, lh) = (
        (lw / want_scale).round() as i64,
        (lh / want_scale).round() as i64,
    );
    let (mx, my) = (m["x"].as_i64().unwrap(), m["y"].as_i64().unwrap());
    assert!(
        band.geo.x >= mx && band.geo.x + band.geo.w <= mx + lw,
        "{band:?} in {lw}x{lh}+{mx}"
    );
    assert!(
        band.geo.y >= my && band.geo.y + band.geo.h <= my + lh,
        "{band:?} in {lw}x{lh}+{my}"
    );
}

#[test]
#[ignore]
fn fractional_scale_monitor() {
    check_band_on_output("1.5", 1.5, 0);
}

#[test]
#[ignore]
fn rotated_monitor() {
    check_band_on_output("1,transform,1", 1.0, 1);
}

#[test]
#[ignore]
fn workspace_moved_to_other_monitor() {
    let _g = reset();
    let (main, out) = add_output("1");
    let a = open("a");
    name(&a, "A", &[]);
    assert_band_on(&a.address);
    // Take workspace 1, window and all, to the other monitor.
    dispatch(&format!("moveworkspacetomonitor 1 {out}"));
    let band = assert_band_on(&a.address);
    assert_eq!(band.monitor, out);
    dispatch(&format!("moveworkspacetomonitor 1 {main}"));
    let band = assert_band_on(&a.address);
    assert_eq!(band.monitor, main);
}

#[test]
#[ignore]
fn special_workspace_on_second_monitor() {
    let _g = reset();
    let (main, out) = add_output("1");
    let a = open("a");
    name(&a, "A", &[]);
    assert_band_on(&a.address);
    // Open a special workspace on the other monitor: a's monitor is untouched.
    dispatch(&format!("focusmonitor {out}"));
    dispatch("togglespecialworkspace other");
    wait_for("special open on second monitor", || {
        (monitor_named(&out)["specialWorkspace"]["id"] != 0).then_some(())
    });
    assert_band_on(&a.address);
    dispatch("togglespecialworkspace other");
    dispatch(&format!("focusmonitor {main}"));
    assert_band_on(&a.address);
}

/// The bug that motivated naming the window wname runs in: an agent in one
/// terminal runs `wname -n` while you type in another window. The band must
/// go on the agent's terminal, not on the window with focus.
#[test]
#[ignore]
fn names_the_window_it_runs_in_not_the_focused_one() {
    let _g = reset();
    let dir = std::env::temp_dir().join(format!("wname-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let go = dir.join("go");
    let _ = std::fs::remove_file(&go);
    let script = format!(
        "while [ ! -e {go} ]; do sleep 0.05; done; {bin} -n Agent > {out} 2>&1; sleep 600",
        go = go.display(),
        bin = env!("CARGO_BIN_EXE_wname"),
        out = dir.join("out").display(),
    );
    let agent = open_running("agent", &["sh", "-c", &script]);
    let typing = open("typing");
    assert_eq!(
        active_address(),
        typing.address,
        "focus is on the other window"
    );
    std::fs::write(&go, b"").unwrap();
    assert_band_on(&agent.address);
    let out = wait_for("wname output", || {
        std::fs::read_to_string(dir.join("out"))
            .ok()
            .filter(|s| !s.is_empty())
    });
    assert!(out.contains("Agent") && !out.contains("focused"), "{out}");
    assert_eq!(active_address(), typing.address, "focus did not move");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore]
fn focused_flag_keeps_the_old_behaviour() {
    let _g = reset();
    let dir = std::env::temp_dir().join(format!("wname-test-f{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let go = dir.join("go");
    let _ = std::fs::remove_file(&go);
    let script = format!(
        "while [ ! -e {go} ]; do sleep 0.05; done; {bin} -n Focused --focused; sleep 600",
        go = go.display(),
        bin = env!("CARGO_BIN_EXE_wname"),
    );
    let _agent = open_running("agent", &["sh", "-c", &script]);
    let typing = open("typing");
    std::fs::write(&go, b"").unwrap();
    assert_band_on(&typing.address);
    let _ = std::fs::remove_dir_all(&dir);
}
