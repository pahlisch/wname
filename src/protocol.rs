//! The control socket between the `wname` command and the band daemon.
//!
//! One JSON request per connection, one JSON reply, then both sides close.

use crate::color::Color;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

pub const IO_TIMEOUT: Duration = Duration::from_secs(3);
/// Titles longer than this are cut. The band ellipsizes anyway; this only
/// bounds what a client can make the daemon store.
pub const MAX_TITLE_CHARS: usize = 256;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Request {
    /// Put a band on a window, or rename it. With no color, keep the current
    /// one or pick an unused one.
    Set {
        address: String,
        title: String,
        color: Option<ColorChoice>,
    },
    Clear {
        address: String,
    },
    ClearAll,
    List,
    Ping,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColorChoice {
    Random,
    Exact(Color),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BandInfo {
    pub address: String,
    pub title: String,
    pub color: Color,
    /// Why the band is not on screen, or None when it is.
    pub hidden: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum Reply {
    Ok {
        #[serde(default)]
        bands: Vec<BandInfo>,
        /// For Set: whether an existing color was kept.
        #[serde(default)]
        kept: bool,
    },
    Error {
        message: String,
    },
}

impl Reply {
    pub fn ok() -> Self {
        Reply::Ok {
            bands: vec![],
            kept: false,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Reply::Error {
            message: message.into(),
        }
    }
}

/// Make a user-supplied title fit on one line.
pub fn clean_title(raw: &str) -> String {
    let one_line: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let squeezed = one_line.split_whitespace().collect::<Vec<_>>().join(" ");
    squeezed.chars().take(MAX_TITLE_CHARS).collect()
}

pub fn state_dir(signature: &str) -> PathBuf {
    crate::hypr::runtime_dir().join("wname").join(signature)
}

pub fn socket_path(signature: &str) -> PathBuf {
    state_dir(signature).join("ctl.sock")
}

pub fn write_message<T: Serialize>(stream: &mut UnixStream, msg: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(msg)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    stream.flush()
}

pub fn read_message<T: for<'de> Deserialize<'de>>(stream: &UnixStream) -> io::Result<T> {
    const LIMIT: u64 = 64 * 1024;
    let mut line = String::new();
    let n = BufReader::new(stream).take(LIMIT).read_line(&mut line)?;
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "empty message",
        ));
    }
    if !line.ends_with('\n') && n as u64 >= LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too long",
        ));
    }
    serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Send one request and wait for the reply.
pub fn call(signature: &str, req: &Request) -> io::Result<Reply> {
    let mut stream = UnixStream::connect(socket_path(signature))?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    write_message(&mut stream, req)?;
    read_message(&stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        let reqs = vec![
            Request::Set {
                address: "0xabc".into(),
                title: "Blog".into(),
                color: None,
            },
            Request::Set {
                address: "0xabc".into(),
                title: "Blog".into(),
                color: Some(ColorChoice::Random),
            },
            Request::Set {
                address: "0xabc".into(),
                title: "Blog".into(),
                color: Some(ColorChoice::Exact(crate::color::parse("red").unwrap())),
            },
            Request::Clear {
                address: "0xabc".into(),
            },
            Request::ClearAll,
            Request::List,
            Request::Ping,
            Request::Quit,
        ];
        for req in reqs {
            let text = serde_json::to_string(&req).unwrap();
            assert_eq!(
                serde_json::from_str::<Request>(&text).unwrap(),
                req,
                "{text}"
            );
        }
    }

    #[test]
    fn replies_round_trip() {
        let reply = Reply::Ok {
            bands: vec![BandInfo {
                address: "0xabc".into(),
                title: "Blog".into(),
                color: crate::color::parse("teal").unwrap(),
                hidden: Some("covered".into()),
            }],
            kept: true,
        };
        let text = serde_json::to_string(&reply).unwrap();
        assert_eq!(serde_json::from_str::<Reply>(&text).unwrap(), reply);
        let err = serde_json::to_string(&Reply::error("nope")).unwrap();
        assert_eq!(err, r#"{"status":"error","message":"nope"}"#);
        assert_eq!(
            serde_json::from_str::<Reply>(r#"{"status":"ok"}"#).unwrap(),
            Reply::ok()
        );
    }

    #[test]
    fn titles_are_one_line() {
        assert_eq!(clean_title("  Blog\n\tproject  "), "Blog project");
        assert_eq!(clean_title("a\u{7}b"), "a b");
        assert_eq!(clean_title("   "), "");
        assert_eq!(clean_title("Café 🚀 <b>x</b>"), "Café 🚀 <b>x</b>");
        let long = "x".repeat(1000);
        assert_eq!(clean_title(&long).chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn socket_round_trip() {
        let (mut a, b) = UnixStream::pair().unwrap();
        write_message(&mut a, &Request::Ping).unwrap();
        assert_eq!(read_message::<Request>(&b).unwrap(), Request::Ping);
    }

    #[test]
    fn empty_and_garbage_messages_fail() {
        let (a, b) = UnixStream::pair().unwrap();
        drop(a);
        assert!(read_message::<Request>(&b).is_err());
        let (mut a, b) = UnixStream::pair().unwrap();
        a.write_all(b"{not json}\n").unwrap();
        assert!(read_message::<Request>(&b).is_err());
    }

    #[test]
    fn oversized_message_is_refused() {
        let (mut a, b) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || {
            let _ = a.write_all(&vec![b'x'; 200 * 1024]);
        });
        let err = read_message::<Request>(&b).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        drop(b);
        writer.join().unwrap();
    }
}
