//! Hyprland IPC: the request socket, the event socket and the JSON they return.
//!
//! Queries go straight to `.socket.sock` instead of spawning `hyprctl`, which
//! keeps a refresh well under a millisecond.

use serde::Deserialize;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const IO_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRef {
    pub id: i64,
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Client {
    pub address: String,
    #[serde(default = "yes")]
    pub mapped: bool,
    #[serde(default)]
    pub hidden: bool,
    /// False for a background tab in a group. Stays true on workspaces not
    /// in view, so it does not replace the workspace check.
    #[serde(default = "yes")]
    pub visible: bool,
    pub at: [i32; 2],
    pub size: [i32; 2],
    #[serde(default)]
    pub workspace: WorkspaceRef,
    #[serde(default)]
    pub floating: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub monitor: i64,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub title: String,
    /// 0 none, 1 maximized, 2 fullscreen, 3 both.
    #[serde(default)]
    pub fullscreen: i32,
    #[serde(default)]
    pub grouped: Vec<String>,
    #[serde(rename = "focusHistoryID", default = "far_back")]
    pub focus_history_id: i64,
    /// The process that owns the window.
    #[serde(default)]
    pub pid: i64,
}

fn yes() -> bool {
    true
}

fn far_back() -> i64 {
    i64::MAX
}

impl Client {
    pub fn label(&self) -> &str {
        if self.title.is_empty() {
            &self.class
        } else {
            &self.title
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Monitor {
    pub id: i64,
    pub name: String,
    pub x: i32,
    pub y: i32,
    /// Pixels in the current mode, before scale and transform.
    pub width: i32,
    pub height: i32,
    #[serde(default = "one")]
    pub scale: f64,
    #[serde(default)]
    pub transform: i32,
    #[serde(default)]
    pub active_workspace: WorkspaceRef,
    #[serde(default)]
    pub special_workspace: WorkspaceRef,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub disabled: bool,
}

fn one() -> f64 {
    1.0
}

impl Monitor {
    /// Size in layout coordinates, which is what window positions use.
    pub fn logical_size(&self) -> (i32, i32) {
        let (w, h) = if self.transform % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        (
            (f64::from(w) / scale).round() as i32,
            (f64::from(h) / scale).round() as i32,
        )
    }
}

/// Addresses come as `0xaaaa…` from JSON and as `aaaa…` in events.
pub fn normalize_address(address: &str) -> String {
    let a = address.trim().to_ascii_lowercase();
    let bare = a.strip_prefix("0x").unwrap_or(&a);
    format!("0x{bare}")
}

pub fn instance_signature() -> Option<String> {
    std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .ok()
        .filter(|s| !s.is_empty() && !s.contains('/'))
}

pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub fn socket_dir(signature: &str) -> PathBuf {
    runtime_dir().join("hypr").join(signature)
}

#[derive(Clone, Debug)]
pub struct Hyprland {
    signature: String,
}

impl Hyprland {
    pub fn new(signature: impl Into<String>) -> Self {
        Self {
            signature: signature.into(),
        }
    }

    pub fn from_env() -> io::Result<Self> {
        instance_signature().map(Self::new).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "HYPRLAND_INSTANCE_SIGNATURE is not set; is Hyprland running?",
            )
        })
    }

    pub fn signature(&self) -> &str {
        &self.signature
    }

    pub fn request(&self, command: &str) -> io::Result<String> {
        let mut stream = UnixStream::connect(socket_dir(&self.signature).join(".socket.sock"))?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream.write_all(command.as_bytes())?;
        let mut out = String::new();
        stream.read_to_string(&mut out)?;
        Ok(out)
    }

    fn json<T: for<'de> Deserialize<'de>>(&self, command: &str) -> io::Result<T> {
        let raw = self.request(command)?;
        serde_json::from_str(&raw).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{command}: {e}: {}",
                    raw.chars().take(120).collect::<String>()
                ),
            )
        })
    }

    pub fn clients(&self) -> io::Result<Vec<Client>> {
        self.json("j/clients")
    }

    pub fn monitors(&self) -> io::Result<Vec<Monitor>> {
        self.json("j/monitors")
    }

    /// The window with keyboard focus. Hyprland answers `{}` when there is none.
    pub fn active_window(&self) -> io::Result<Option<Client>> {
        let raw = self.request("j/activewindow")?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if value.get("address").is_none() {
            return Ok(None);
        }
        serde_json::from_value(value)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn cursor(&self) -> io::Result<(i64, i64)> {
        #[derive(Deserialize)]
        struct Pos {
            x: i64,
            y: i64,
        }
        let p: Pos = self.json("j/cursorpos")?;
        Ok((p.x, p.y))
    }

    pub fn animation_durations(&self) -> io::Result<crate::anim::Durations> {
        let master = self.int_option("animations:enabled")?.unwrap_or(1) != 0;
        let raw = self.request("j/animations")?;
        Ok(crate::anim::Durations::parse(&raw, master))
    }

    /// An integer config option, or None if Hyprland does not know it.
    pub fn int_option(&self, name: &str) -> io::Result<Option<i64>> {
        let raw = self.request(&format!("j/getoption {name}"))?;
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        Ok(value.get("int").and_then(serde_json::Value::as_i64))
    }

    pub fn events(&self) -> io::Result<UnixStream> {
        UnixStream::connect(socket_dir(&self.signature).join(".socket2.sock"))
    }
}

/// The events that matter to the bands. Everything else is `Other`.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    CloseWindow {
        address: String,
    },
    /// Anything that can move, show, hide or restack a window.
    Layout,
    /// Title and focus-free noise that cannot change placement.
    Ignored,
}

pub fn parse_event(line: &str) -> Option<Event> {
    let (name, data) = line.split_once(">>")?;
    Some(match name {
        "closewindow" => Event::CloseWindow {
            address: normalize_address(data),
        },
        "windowtitle" | "windowtitlev2" | "activelayout" | "submap" | "bell" | "urgent"
        | "screencast" | "screencastv2" | "ignoregrouplock" | "lockgroups" => Event::Ignored,
        _ => Event::Layout,
    })
}

/// Splits a byte stream into complete event lines, keeping any partial tail.
#[derive(Default)]
pub struct LineBuffer {
    buf: Vec<u8>,
}

impl LineBuffer {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
            if !text.is_empty() {
                lines.push(text);
            }
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: &str = r#"{
        "address": "0xaaaaf6154b00", "mapped": true, "hidden": false, "visible": true,
        "acceptsInput": true, "at": [12, 38], "size": [941, 508],
        "workspace": {"id": 3, "name": "3"}, "floating": false, "monitor": 0,
        "class": "foot", "title": "user@host:~", "initialClass": "foot", "initialTitle": "foot",
        "pid": 1207309, "xwayland": false, "pinned": false, "pinFullscreened": false,
        "fullscreen": 0, "fullscreenClient": 0, "fullscreenHandler": "default",
        "allowedOverFullscreen": true, "grouped": [], "tags": ["terminal*"],
        "swallowing": "0x0", "focusHistoryID": 10, "inhibitingIdle": false,
        "xdgTag": "", "xdgDescription": "", "contentType": "none", "tearingHint": false,
        "stableId": "18000018"
    }"#;

    const MONITOR: &str = r#"{
        "id": 0, "name": "Virtual-1", "description": "Red Hat Inc. QEMU Monitor",
        "width": 1920, "height": 1080, "refreshRate": 60.098, "x": 0, "y": 0,
        "activeWorkspace": {"id": 1, "name": "1"}, "specialWorkspace": {"id": 0, "name": ""},
        "reserved": [0, 26, 0, 0], "scale": 1.00, "transform": 0, "focused": true,
        "dpmsStatus": true, "vrr": false, "disabled": false
    }"#;

    #[test]
    fn parses_hyprland_0_56_client() {
        let c: Client = serde_json::from_str(CLIENT).unwrap();
        assert_eq!(c.address, "0xaaaaf6154b00");
        assert_eq!(c.at, [12, 38]);
        assert_eq!(c.size, [941, 508]);
        assert_eq!(c.workspace.id, 3);
        assert_eq!(c.focus_history_id, 10);
        assert!(c.mapped && c.visible && !c.hidden && !c.floating);
        assert_eq!(c.label(), "user@host:~");
    }

    #[test]
    fn client_tolerates_missing_optional_fields() {
        let c: Client =
            serde_json::from_str(r#"{"address":"0x1","at":[0,0],"size":[10,10]}"#).unwrap();
        assert!(c.mapped && c.visible);
        assert_eq!(c.focus_history_id, i64::MAX);
        assert_eq!(c.label(), "");
    }

    #[test]
    fn parses_monitor() {
        let m: Monitor = serde_json::from_str(MONITOR).unwrap();
        assert_eq!(m.name, "Virtual-1");
        assert_eq!(m.active_workspace.id, 1);
        assert_eq!(m.special_workspace.id, 0);
        assert_eq!(m.logical_size(), (1920, 1080));
    }

    #[test]
    fn logical_size_handles_scale_and_rotation() {
        let mut m: Monitor = serde_json::from_str(MONITOR).unwrap();
        m.scale = 1.5;
        assert_eq!(m.logical_size(), (1280, 720));
        m.scale = 2.0;
        m.transform = 1;
        assert_eq!(m.logical_size(), (540, 960));
        m.transform = 3;
        assert_eq!(m.logical_size(), (540, 960));
        m.transform = 2;
        assert_eq!(m.logical_size(), (960, 540));
        m.scale = 0.0;
        m.transform = 0;
        assert_eq!(m.logical_size(), (1920, 1080));
    }

    #[test]
    fn address_normalization() {
        assert_eq!(normalize_address("aaaaf6154b00"), "0xaaaaf6154b00");
        assert_eq!(normalize_address("0xAAAAF6154B00"), "0xaaaaf6154b00");
        assert_eq!(normalize_address(" 0xabc\n"), "0xabc");
    }

    #[test]
    fn events() {
        assert_eq!(
            parse_event("closewindow>>aaab03bda350"),
            Some(Event::CloseWindow {
                address: "0xaaab03bda350".into()
            })
        );
        for layout in [
            "workspacev2>>2,2",
            "activespecialv2>>-98,special:sp,WAYLAND-1",
            "movewindowv2>>aaab03c04470,-98,special:sp",
            "fullscreen>>1",
            "changefloatingmode>>aaab03c04470,1",
            "monitoraddedv2>>1,HEADLESS-1,",
            "openwindow>>aaab03bda350,1,foot,A",
            "activewindowv2>>",
            "pin>>aaab03c04470,1",
            "someneweventv9>>x",
        ] {
            assert_eq!(parse_event(layout), Some(Event::Layout), "{layout}");
        }
        assert_eq!(
            parse_event("windowtitlev2>>abc,title >> with arrows"),
            Some(Event::Ignored)
        );
        assert_eq!(parse_event("garbage"), None);
    }

    #[test]
    fn line_buffer_keeps_partial_lines() {
        let mut b = LineBuffer::default();
        assert!(b.push(b"workspacev2>>1,").is_empty());
        assert_eq!(
            b.push(b"1\nclosewindow>>ab\n\nfull"),
            vec!["workspacev2>>1,1", "closewindow>>ab"]
        );
        assert_eq!(b.push(b"screen>>0\n"), vec!["fullscreen>>0"]);
        assert!(b.push(b"").is_empty());
    }

    #[test]
    fn line_buffer_survives_invalid_utf8() {
        let mut b = LineBuffer::default();
        let lines = b.push(b"windowtitlev2>>a,\xff\xfe\n");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("windowtitlev2>>a,"));
    }
}
