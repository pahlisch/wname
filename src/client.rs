//! The command-line side: reach the daemon, starting it when needed.

use crate::protocol::{self, Reply, Request};
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const START_TIMEOUT: Duration = Duration::from_secs(5);

fn not_running(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

/// Send a request to a running daemon. `Ok(None)` when none is running.
pub fn call_existing(signature: &str, req: &Request) -> io::Result<Option<Reply>> {
    match protocol::call(signature, req) {
        Ok(reply) => Ok(Some(reply)),
        Err(e) if not_running(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Send a request, starting the daemon first if it is not running.
pub fn call(signature: &str, req: &Request) -> io::Result<Reply> {
    if let Some(reply) = call_existing(signature, req)? {
        return Ok(reply);
    }
    spawn_daemon(signature)?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        match protocol::call(signature, &Request::Ping) {
            Ok(_) => return protocol::call(signature, req),
            Err(e) if not_running(&e) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                let log = protocol::state_dir(signature).join("daemon.log");
                return Err(io::Error::new(
                    e.kind(),
                    format!("the band daemon did not start ({e}); see {}", log.display()),
                ));
            }
        }
    }
}

fn spawn_daemon(signature: &str) -> io::Result<()> {
    let dir = protocol::state_dir(signature);
    fs::create_dir_all(&dir)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("daemon.log"))?;
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("--daemon")
        .env("HYPRLAND_INSTANCE_SIGNATURE", signature)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    // SAFETY: setsid is async-signal-safe. A new session keeps the daemon
    // alive when the terminal that started it closes.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map(drop)
}
