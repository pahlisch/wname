//! The band daemon: one process that draws every band on one Hyprland
//! instance.
//!
//! It wakes on Hyprland events (workspace switches, moves, closes, …) and
//! also samples window geometry on a short timer, because Hyprland sends no
//! event while a window is dragged or resized. Samples use the IPC socket
//! directly, so a tick costs a couple of small reads, not two `hyprctl`
//! processes.
//!
//! It exits on its own a moment after the last band goes away.

use crate::anim::{self, Before, Durations};
use crate::color::{self, Color};
use crate::hypr::{self, Event, Hyprland, LineBuffer};
use crate::layout::{self, Hidden, Margins, Placement};
use crate::protocol::{self, BandInfo, ColorChoice, Reply, Request};
use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const NAMESPACE: &str = "wname";
const FONT_FAMILY: &str = "\"JetBrainsMono Nerd Font\", \"JetBrains Mono\", monospace";
const FONT_PX: u32 = 18;
/// Sample geometry this often while something is moving…
const FAST_TICK: Duration = Duration::from_millis(16);
/// …and this often otherwise.
const SLOW_TICK: Duration = Duration::from_millis(100);
/// How long to keep sampling fast after the last change.
const SETTLE: Duration = Duration::from_millis(400);
/// How long to linger with no bands before exiting.
const IDLE_EXIT: Duration = Duration::from_secs(3);
fn debug_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WNAME_DEBUG").is_some_and(|v| !v.is_empty()))
}

/// Log to the daemon's stderr (its log file) when WNAME_DEBUG is set.
macro_rules! debug {
    ($($arg:tt)*) => {
        if debug_enabled() {
            eprintln!("wname[{:?}]: {}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() % 100_000,
                format!($($arg)*));
        }
    };
}

/// Monitors only change along with an event, but ask again now and then
/// in case one was missed.
const MONITOR_CACHE: Duration = Duration::from_secs(2);
/// How long to trust the animation settings before asking again.
const ANIM_CACHE: Duration = Duration::from_secs(2);
/// Consecutive failed Hyprland queries before giving up.
const MAX_QUERY_FAILURES: u32 = 40;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Saved {
    address: String,
    title: String,
    color: Color,
}

struct Band {
    id: u32,
    title: String,
    color: Color,
    window: gtk::Window,
    label: gtk::Label,
    monitor: Option<String>,
    margins: Option<Margins>,
    visible: bool,
    status: Option<Hidden>,
    /// Where the band was headed at the last refresh.
    before: Before,
    /// Kept hidden until then, while the window animates into place.
    hold_until: Option<Instant>,
}

impl Band {
    fn info(&self, address: &str) -> BandInfo {
        BandInfo {
            address: address.to_string(),
            title: self.title.clone(),
            color: self.color.clone(),
            hidden: if self.visible {
                None
            } else {
                Some(
                    self.status
                        .unwrap_or(Hidden::OtherWorkspace)
                        .describe()
                        .to_string(),
                )
            },
        }
    }
}

struct Daemon {
    hypr: Hyprland,
    bands: BTreeMap<String, Band>,
    next_id: u32,
    bar_h: i32,
    css: gtk::CssProvider,
    main_loop: glib::MainLoop,
    state_file: PathBuf,
    refresh_queued: bool,
    monitors: Option<(Instant, Vec<hypr::Monitor>)>,
    last_change: Instant,
    last_refresh: Instant,
    failures: u32,
    idle_since: Option<Instant>,
    events: LineBuffer,
    cursor: Option<(i64, i64)>,
    anim: Option<(Instant, Durations)>,
}

type Shared = Rc<RefCell<Daemon>>;

pub fn run(signature: String) -> io::Result<()> {
    let dir = protocol::state_dir(&signature);
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;

    // One daemon per Hyprland instance. The lock lives as long as we do.
    let lock = File::create(dir.join("daemon.lock"))?;
    // SAFETY: flock on a descriptor we own.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        eprintln!("wname: a daemon is already running for this Hyprland instance");
        return Ok(());
    }

    let sock_path = protocol::socket_path(&signature);
    let _ = fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path)?;
    listener.set_nonblocking(true)?;

    let hypr = Hyprland::new(signature.clone());
    let events = hypr.events()?;
    events.set_nonblocking(true)?;

    // A band is a flat color and a line of text. The cairo renderer draws
    // that without loading the GPU drivers, which saves most of the memory.
    if std::env::var_os("GSK_RENDERER").is_none() {
        // SAFETY: still single-threaded; GTK has not started.
        unsafe { std::env::set_var("GSK_RENDERER", "cairo") };
    }
    gtk::init().map_err(|e| io::Error::other(format!("GTK: {e}")))?;
    if !gtk4_layer_shell::is_supported() {
        return Err(io::Error::other(
            "the compositor does not support wlr-layer-shell",
        ));
    }

    let display = gdk::Display::default().ok_or_else(|| io::Error::other("no display"))?;
    let css = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    css.load_from_string(&stylesheet(&BTreeMap::new()));
    let bar_h = measure_bar_height();

    let now = Instant::now();
    let daemon: Shared = Rc::new(RefCell::new(Daemon {
        hypr,
        bands: BTreeMap::new(),
        next_id: 0,
        bar_h,
        css,
        main_loop: glib::MainLoop::new(None, false),
        state_file: dir.join("bands.json"),
        refresh_queued: false,
        monitors: None,
        last_change: now,
        last_refresh: now,
        failures: 0,
        idle_since: Some(now),
        events: LineBuffer::default(),
        cursor: None,
        anim: None,
    }));

    restore(&daemon);
    watch_events(&daemon, events);
    watch_control(&daemon, listener);
    start_ticker(&daemon);
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        let d = daemon.clone();
        glib_unix::unix_signal_add_local(signal, move || {
            d.borrow().main_loop.quit();
            glib::ControlFlow::Break
        });
    }

    let main_loop = daemon.borrow().main_loop.clone();
    main_loop.run();

    let _ = fs::remove_file(&sock_path);
    let mut d = daemon.borrow_mut();
    for (_, band) in std::mem::take(&mut d.bands) {
        band.window.destroy();
    }
    drop(lock);
    Ok(())
}

fn stylesheet(bands: &BTreeMap<String, Band>) -> String {
    let mut css = format!(
        "window.wname-band {{ background: transparent; }}\n\
         window.wname-band label {{\n  font-family: {FONT_FAMILY};\n  font-size: {FONT_PX}px;\n  \
         font-weight: 700;\n  padding: 0 6px;\n  margin: 0;\n  min-height: 0;\n}}\n"
    );
    for band in bands.values() {
        // Colors were validated as #rrggbb, so they are safe to interpolate.
        css.push_str(&format!(
            "window.wname-b{id} {{ background-color: {bg}; }}\nwindow.wname-b{id} label {{ color: {fg}; }}\n",
            id = band.id,
            bg = band.color.bg,
            fg = band.color.fg,
        ));
    }
    css
}

/// The height GTK will give a one-line label in the band font. Using it as
/// the band height keeps the margins and the real surface size in agreement.
fn measure_bar_height() -> i32 {
    let probe = gtk::Window::new();
    probe.add_css_class("wname-band");
    let label = gtk::Label::new(Some("Ag|"));
    probe.set_child(Some(&label));
    let (_, natural, _, _) = label.measure(gtk::Orientation::Vertical, -1);
    probe.destroy();
    natural.max(1)
}

fn find_gdk_monitor(name: &str) -> Option<gdk::Monitor> {
    let monitors = gdk::Display::default()?.monitors();
    (0..monitors.n_items())
        .filter_map(|i| monitors.item(i)?.downcast::<gdk::Monitor>().ok())
        .find(|m| m.connector().as_deref() == Some(name))
}

fn new_band_window(title: &str, id: u32) -> (gtk::Window, gtk::Label) {
    let window = gtk::Window::new();
    window.set_title(Some(title));
    window.set_decorated(false);
    window.add_css_class("wname-band");
    window.add_css_class(&format!("wname-b{id}"));

    let label = gtk::Label::new(Some(title));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_single_line_mode(true);
    label.set_halign(gtk::Align::Center);
    label.set_valign(gtk::Align::Center);
    label.set_hexpand(true);
    window.set_child(Some(&label));

    window.init_layer_shell();
    // Top is above every window, which is all the band needs, and below
    // overlay surfaces such as menus, launchers and notifications.
    window.set_layer(Layer::Top);
    window.set_namespace(Some(NAMESPACE));
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::None);
    for edge in [Edge::Top, Edge::Left, Edge::Right, Edge::Bottom] {
        window.set_anchor(edge, true);
    }
    // Clicks go through to the window underneath.
    window.connect_realize(|w| {
        if let Some(surface) = w.surface() {
            surface.set_input_region(Some(&gtk::cairo::Region::create()));
        }
    });
    (window, label)
}

impl Daemon {
    fn in_use_colors(&self, except: Option<&str>) -> Vec<String> {
        self.bands
            .iter()
            .filter(|(addr, _)| Some(addr.as_str()) != except)
            .map(|(_, b)| b.color.bg.clone())
            .collect()
    }

    fn restyle(&self) {
        self.css.load_from_string(&stylesheet(&self.bands));
    }

    fn save(&self) {
        let saved: Vec<Saved> = self
            .bands
            .iter()
            .map(|(address, b)| Saved {
                address: address.clone(),
                title: b.title.clone(),
                color: b.color.clone(),
            })
            .collect();
        let result = if saved.is_empty() {
            match fs::remove_file(&self.state_file) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        } else {
            let tmp = self.state_file.with_extension("json.tmp");
            serde_json::to_vec_pretty(&saved)
                .map_err(io::Error::from)
                .and_then(|bytes| fs::write(&tmp, bytes))
                .and_then(|()| fs::rename(&tmp, &self.state_file))
        };
        if let Err(e) = result {
            eprintln!("wname: could not save bands: {e}");
        }
    }

    fn set(&mut self, address: String, title: String, color: Color) {
        if let Some(band) = self.bands.get_mut(&address) {
            band.title = title.clone();
            band.color = color;
            band.label.set_text(&title);
            band.window.set_title(Some(&title));
        } else {
            let id = self.next_id;
            self.next_id += 1;
            let (window, label) = new_band_window(&title, id);
            self.bands.insert(
                address,
                Band {
                    id,
                    title,
                    color,
                    window,
                    label,
                    monitor: None,
                    margins: None,
                    visible: false,
                    status: None,
                    before: Before::New,
                    hold_until: None,
                },
            );
        }
        self.idle_since = None;
        self.restyle();
        self.save();
    }

    fn remove(&mut self, address: &str) -> Option<BandInfo> {
        let band = self.bands.remove(address)?;
        let info = band.info(address);
        band.window.destroy();
        if self.bands.is_empty() {
            self.idle_since = Some(Instant::now());
        }
        self.restyle();
        self.save();
        Some(info)
    }

    fn remove_all(&mut self) -> Vec<BandInfo> {
        let addresses: Vec<String> = self.bands.keys().cloned().collect();
        addresses.iter().filter_map(|a| self.remove(a)).collect()
    }

    /// Query Hyprland and move, show or hide every band.
    fn refresh(&mut self) {
        self.refresh_queued = false;
        self.last_refresh = Instant::now();
        if self.bands.is_empty() {
            return;
        }
        let now = Instant::now();
        let cached = match &self.monitors {
            Some((at, m)) if now.duration_since(*at) < MONITOR_CACHE => Some(m.clone()),
            _ => None,
        };
        let snapshot = self.hypr.clients().and_then(|c| {
            let m = match cached {
                Some(m) => m,
                None => {
                    let m = self.hypr.monitors()?;
                    self.monitors = Some((now, m.clone()));
                    m
                }
            };
            Ok((c, m))
        });
        let (mut clients, monitors) = match snapshot {
            Ok(s) => s,
            Err(e) => {
                self.failures += 1;
                if self.failures >= MAX_QUERY_FAILURES {
                    eprintln!("wname: Hyprland is not answering ({e}); exiting");
                    self.main_loop.quit();
                }
                return;
            }
        };
        self.failures = 0;
        for c in &mut clients {
            c.address = hypr::normalize_address(&c.address);
        }

        // A move made while the pointer moves too is a drag, not an animation.
        let cursor = self.hypr.cursor().ok();
        let pointer_moved = cursor.is_none() || self.cursor.is_none() || cursor != self.cursor;
        self.cursor = cursor.or(self.cursor);

        let Daemon {
            bands,
            hypr,
            anim: anim_cache,
            bar_h,
            ..
        } = self;
        let mut durations = || -> Durations {
            match anim_cache {
                Some((at, d)) if now.duration_since(*at) < ANIM_CACHE => *d,
                _ => {
                    let d = hypr.animation_durations().unwrap_or_default();
                    *anim_cache = Some((now, d));
                    d
                }
            }
        };

        let mut gone = Vec::new();
        let mut changed = false;
        for (address, band) in bands.iter_mut() {
            match layout::place(address, &clients, &monitors, *bar_h) {
                Placement::Gone => gone.push(address.clone()),
                Placement::Hidden(why) => {
                    if band.before != Before::Hidden(why) {
                        debug!("{address}: {:?} -> hidden ({why:?})", band.before);
                    }
                    band.before = Before::Hidden(why);
                    band.hold_until = None;
                    changed |= hide(band, why);
                }
                Placement::Shown(shown) => {
                    let next = Before::Shown(shown.global);
                    if band.before != next {
                        let onto_special = clients
                            .iter()
                            .find(|c| &c.address == address)
                            .is_some_and(|c| c.workspace.id < 0);
                        let wait = anim::hold(
                            band.before,
                            shown.global,
                            onto_special,
                            pointer_moved,
                            &durations(),
                        );
                        debug!(
                            "{address}: {:?} -> {:?}, pointer moved {pointer_moved}, hold {wait:?}",
                            band.before, shown.global
                        );
                        if !wait.is_zero() {
                            band.hold_until = Some(now + wait);
                        }
                        band.before = next;
                    }
                    if band.hold_until.is_some_and(|t| now < t) {
                        changed |= hide(band, Hidden::Animating);
                    } else {
                        band.hold_until = None;
                        changed |= show(band, &shown);
                    }
                }
            }
        }
        for address in gone {
            self.remove(&address);
            changed = true;
        }
        if changed {
            self.last_change = Instant::now();
        }
    }
}

fn hide(band: &mut Band, why: Hidden) -> bool {
    band.status = Some(why);
    if !band.visible {
        return false;
    }
    band.window.set_visible(false);
    band.visible = false;
    true
}

fn show(band: &mut Band, shown: &layout::Shown) -> bool {
    let mut changed = false;
    let Some(gmon) = find_gdk_monitor(&shown.monitor) else {
        // GTK has not seen this output yet; try again next tick.
        return hide(band, Hidden::NoMonitor);
    };
    // Compare the output itself, not only its name: an output that was
    // unplugged and plugged back, or recreated, has the same name.
    let same_output = band.monitor.as_deref() == Some(shown.monitor.as_str())
        && band.window.monitor().as_ref() == Some(&gmon);
    if !same_output {
        // Moving a mapped layer surface to another output leaves it without
        // a frame on the new one (0x0 in `hyprctl layers`). Unmap, move, and
        // map again, as on first show.
        if band.visible {
            band.window.set_visible(false);
            band.visible = false;
        }
        band.window.set_monitor(Some(&gmon));
        band.monitor = Some(shown.monitor.clone());
        changed = true;
    }
    if band.margins != Some(shown.margins) {
        let m = shown.margins;
        band.window.set_margin(Edge::Left, m.left);
        band.window.set_margin(Edge::Top, m.top);
        band.window.set_margin(Edge::Right, m.right);
        band.window.set_margin(Edge::Bottom, m.bottom);
        band.margins = Some(m);
        changed = true;
    }
    band.status = None;
    if !band.visible {
        band.window.set_visible(true);
        band.visible = true;
        changed = true;
    }
    changed
}

fn queue_refresh(daemon: &Shared) {
    let mut d = daemon.borrow_mut();
    if d.refresh_queued {
        return;
    }
    d.refresh_queued = true;
    let weak = Rc::downgrade(daemon);
    glib::idle_add_local_full(glib::Priority::HIGH, move || {
        if let Some(d) = weak.upgrade() {
            d.borrow_mut().refresh();
        }
        glib::ControlFlow::Break
    });
}

fn restore(daemon: &Shared) {
    let path = daemon.borrow().state_file.clone();
    let Ok(bytes) = fs::read(&path) else { return };
    let Ok(saved) = serde_json::from_slice::<Vec<Saved>>(&bytes) else {
        eprintln!("wname: ignoring unreadable {}", path.display());
        return;
    };
    let mut d = daemon.borrow_mut();
    for s in saved {
        // Colors from disk are checked like any other input.
        let Some(color) = checked_color(&s.color) else {
            continue;
        };
        let title = protocol::clean_title(&s.title);
        if title.is_empty() {
            continue;
        }
        d.set(hypr::normalize_address(&s.address), title, color);
    }
    // Windows that closed while no daemon was running drop out on refresh.
    d.refresh();
}

fn checked_color(c: &Color) -> Option<Color> {
    let bg = color::parse_hex(&c.bg)?;
    let fg = color::parse_hex(&c.fg)?;
    let label = color::named(&c.label)
        .filter(|n| n.bg == bg)
        .map(|n| n.label)
        .unwrap_or_else(|| bg.clone());
    Some(Color { label, bg, fg })
}

fn watch_events(daemon: &Shared, mut stream: UnixStream) {
    let fd = stream.as_raw_fd();
    let d = daemon.clone();
    glib_unix::unix_fd_add_local(
        fd,
        glib::IOCondition::IN | glib::IOCondition::HUP | glib::IOCondition::ERR,
        move |_, _| {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => {
                        eprintln!("wname: Hyprland closed the event socket; exiting");
                        d.borrow().main_loop.quit();
                        return glib::ControlFlow::Break;
                    }
                    Ok(n) => {
                        let lines = d.borrow_mut().events.push(&buf[..n]);
                        for line in lines {
                            match hypr::parse_event(&line) {
                                Some(Event::CloseWindow { address }) => {
                                    d.borrow_mut().remove(&address);
                                }
                                Some(Event::Layout) => {
                                    // Workspace and monitor changes come as events.
                                    d.borrow_mut().monitors = None;
                                    queue_refresh(&d);
                                }
                                Some(Event::Ignored) | None => {}
                            }
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        return glib::ControlFlow::Continue;
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        eprintln!("wname: event socket: {e}; exiting");
                        d.borrow().main_loop.quit();
                        return glib::ControlFlow::Break;
                    }
                }
            }
        },
    );
}

fn watch_control(daemon: &Shared, listener: UnixListener) {
    let fd = listener.as_raw_fd();
    let d = daemon.clone();
    glib_unix::unix_fd_add_local(fd, glib::IOCondition::IN, move |_, _| {
        loop {
            match listener.accept() {
                Ok((stream, _)) => serve(&d, stream),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    eprintln!("wname: control socket: {e}");
                    break;
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

fn serve(daemon: &Shared, mut stream: UnixStream) {
    // The listener is non-blocking; this connection should not be.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let reply = match protocol::read_message::<Request>(&stream) {
        Ok(req) => handle(daemon, req),
        Err(e) => Reply::error(format!("bad request: {e}")),
    };
    let _ = protocol::write_message(&mut stream, &reply);
}

fn handle(daemon: &Shared, req: Request) -> Reply {
    let reply = match req {
        Request::Ping => Reply::ok(),
        Request::List => {
            let d = daemon.borrow();
            Reply::Ok {
                bands: d.bands.iter().map(|(a, b)| b.info(a)).collect(),
                kept: false,
            }
        }
        Request::Quit => {
            let mut d = daemon.borrow_mut();
            d.remove_all();
            d.main_loop.quit();
            Reply::ok()
        }
        Request::ClearAll => Reply::Ok {
            bands: daemon.borrow_mut().remove_all(),
            kept: false,
        },
        Request::Clear { address } => Reply::Ok {
            bands: daemon
                .borrow_mut()
                .remove(&hypr::normalize_address(&address))
                .into_iter()
                .collect(),
            kept: false,
        },
        Request::Set {
            address,
            title,
            color,
        } => set_band(daemon, address, title, color),
    };
    queue_refresh(daemon);
    reply
}

fn set_band(daemon: &Shared, address: String, title: String, choice: Option<ColorChoice>) -> Reply {
    let address = hypr::normalize_address(&address);
    let title = protocol::clean_title(&title);
    if title.is_empty() {
        return Reply::error("the name is empty");
    }
    let mut d = daemon.borrow_mut();
    match d.hypr.clients() {
        Ok(clients)
            if clients
                .iter()
                .any(|c| hypr::normalize_address(&c.address) == address) => {}
        Ok(_) => return Reply::error(format!("no window with address {address}")),
        Err(e) => return Reply::error(format!("cannot reach Hyprland: {e}")),
    }
    let current = d.bands.get(&address).map(|b| b.color.clone());
    let (color, kept) = match (choice, current) {
        (Some(ColorChoice::Exact(c)), _) => match checked_color(&c) {
            Some(c) => (c, false),
            None => return Reply::error(format!("invalid color {:?}", c.bg)),
        },
        (None, Some(current)) => (current, true),
        (Some(ColorChoice::Random), _) | (None, None) => (
            color::pick_random(&d.in_use_colors(Some(&address)), color::random_seed()),
            false,
        ),
    };
    d.set(address.clone(), title, color);
    // Place it now, so the reply and an immediate --list are accurate.
    d.refresh();
    let Some(info) = d.bands.get(&address).map(|b| b.info(&address)) else {
        return Reply::error(format!("the window {address} closed"));
    };
    Reply::Ok {
        bands: vec![info],
        kept,
    }
}

fn start_ticker(daemon: &Shared) {
    let weak = Rc::downgrade(daemon);
    glib::timeout_add_local(FAST_TICK, move || {
        let Some(d) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let mut d = d.borrow_mut();
        let now = Instant::now();
        if let Some(since) = d.idle_since
            && d.bands.is_empty()
            && now.duration_since(since) >= IDLE_EXIT
        {
            d.main_loop.quit();
            return glib::ControlFlow::Break;
        }
        let moving = now.duration_since(d.last_change) < SETTLE;
        let hold_over = d
            .bands
            .values()
            .any(|b| b.hold_until.is_some_and(|t| now >= t));
        if moving || hold_over || now.duration_since(d.last_refresh) >= SLOW_TICK {
            d.refresh();
        }
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_color_rejects_bad_hex() {
        let bad = Color {
            label: "x".into(),
            bg: "red;} * {".into(),
            fg: "#000000".into(),
        };
        assert!(checked_color(&bad).is_none());
        let bad_fg = Color {
            label: "x".into(),
            bg: "#000000".into(),
            fg: "url(x)".into(),
        };
        assert!(checked_color(&bad_fg).is_none());
    }

    #[test]
    fn checked_color_fixes_labels() {
        let lying = Color {
            label: "orange".into(),
            bg: "#123456".into(),
            fg: "#FFFFFF".into(),
        };
        let c = checked_color(&lying).unwrap();
        assert_eq!(c.label, "#123456");
        assert_eq!(c.fg, "#ffffff");
        let honest = color::parse("teal").unwrap();
        assert_eq!(checked_color(&honest).unwrap(), honest);
    }

    #[test]
    fn saved_state_round_trips() {
        let s = vec![Saved {
            address: "0xabc".into(),
            title: "Blog".into(),
            color: color::parse("red").unwrap(),
        }];
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Vec<Saved>>(&text).unwrap(), s);
    }
}
