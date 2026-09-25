use clap::{ArgGroup, Parser};
use std::io::{self, IsTerminal};
use std::process::ExitCode;
use wname::color;
use wname::hypr::{self, Hyprland};
use wname::protocol::{BandInfo, ColorChoice, Reply, Request};
use wname::target::{self, Owner};
use wname::{client, daemon};

/// Put a large, colored name band on a Hyprland window.
///
/// The band goes on the window wname is run from: the terminal it runs in,
/// even when another window has focus. Run from a keybinding, where no
/// window started it, it goes on the focused window. It follows the window
/// when it moves or resizes, and hides when the window is out of view.
#[derive(Parser, Debug)]
#[command(
    name = "wname",
    version,
    group(ArgGroup::new("action").args(["name", "clear", "clear_all", "list", "colors", "quit"])),
    after_help = "Examples:\n  wname -n \"Blog project\"\n  wname -n \"Blog project\" -c red\n  wname --clear\n  wname --list"
)]
struct Cli {
    /// Name to show on the window.
    #[arg(short, long, value_name = "TEXT")]
    name: Option<String>,

    /// Band color: a palette name, "random", #rgb or #rrggbb. Without it an
    /// existing band keeps its color and a new one gets an unused color.
    #[arg(short, long, value_name = "COLOR", requires = "name")]
    color: Option<String>,

    /// Act on this window address.
    #[arg(short, long, value_name = "ADDRESS", conflicts_with_all = ["clear_all", "list", "colors", "quit", "focused"])]
    window: Option<String>,

    /// Act on the focused window, even when wname runs in another one.
    #[arg(long, conflicts_with_all = ["clear_all", "list", "colors", "quit"])]
    focused: bool,

    /// Remove the band from the window.
    #[arg(long)]
    clear: bool,

    /// Remove every band.
    #[arg(long)]
    clear_all: bool,

    /// Show every band.
    #[arg(short, long)]
    list: bool,

    /// Show the named colors.
    #[arg(long)]
    colors: bool,

    /// Stop the band daemon, removing every band.
    #[arg(long)]
    quit: bool,

    /// Run the band daemon (started automatically).
    #[arg(long, hide = true, exclusive = true)]
    daemon: bool,
}

type Result<T> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("wname: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    if cli.colors {
        print_colors();
        return Ok(());
    }
    let hypr = Hyprland::from_env().map_err(|e| e.to_string())?;
    let sig = hypr.signature().to_string();

    if cli.daemon {
        return daemon::run(sig).map_err(|e| format!("daemon: {e}"));
    }
    if cli.list {
        return list(&hypr);
    }
    if cli.quit {
        return match existing(&sig, &Request::Quit)? {
            Some(_) => {
                println!("Stopped the band daemon.");
                Ok(())
            }
            None => {
                println!("The band daemon is not running.");
                Ok(())
            }
        };
    }
    if cli.clear_all {
        let removed = existing(&sig, &Request::ClearAll)?.unwrap_or_default();
        match removed.len() {
            0 => println!("No bands to clear."),
            1 => println!("Cleared 1 band."),
            n => println!("Cleared {n} bands."),
        }
        return Ok(());
    }

    let target = target_window(&hypr, cli.window.as_deref(), cli.focused)?;
    let address = hypr::normalize_address(&target.address);

    if cli.clear {
        let removed = existing(&sig, &Request::Clear { address })?.unwrap_or_default();
        if removed.is_empty() {
            println!("No band on “{}”.", target.label());
        } else {
            println!("Cleared the band on “{}”.", target.label());
        }
        return Ok(());
    }

    let Some(name) = cli.name else {
        return Err("nothing to do; try: wname -n \"Blog\" (or --help)".into());
    };
    let choice = match cli.color.as_deref() {
        None => None,
        Some(spec) if color::is_random(spec) => Some(ColorChoice::Random),
        Some(spec) => Some(ColorChoice::Exact(
            color::parse(spec).map_err(|e| e.to_string())?,
        )),
    };
    let reply = client::call(
        &sig,
        &Request::Set {
            address,
            title: name,
            color: choice,
        },
    )
    .map_err(|e| e.to_string())?;
    let (bands, kept) = ok(reply)?;
    let band = bands.first().ok_or("the daemon sent no band back")?;
    let note = if kept {
        format!("{}, kept", band.color.label)
    } else {
        band.color.label.clone()
    };
    println!("Named “{}” as “{}” in {note}.", target.label(), band.title);
    Ok(())
}

fn ok(reply: Reply) -> Result<(Vec<BandInfo>, bool)> {
    match reply {
        Reply::Ok { bands, kept } => Ok((bands, kept)),
        Reply::Error { message } => Err(message),
    }
}

/// Ask a running daemon; `None` when there is none, which for clear, list
/// and quit simply means there are no bands.
fn existing(sig: &str, req: &Request) -> Result<Option<Vec<BandInfo>>> {
    match client::call_existing(sig, req).map_err(|e| e.to_string())? {
        None => Ok(None),
        Some(reply) => ok(reply).map(|(bands, _)| Some(bands)),
    }
}

fn target_window(hypr: &Hyprland, address: Option<&str>, focused: bool) -> Result<hypr::Client> {
    let unreachable = |e: io::Error| format!("cannot reach Hyprland: {e}");
    let focused_window = || {
        hypr.active_window()
            .map_err(unreachable)?
            .ok_or_else(|| "no focused window".to_string())
    };
    if let Some(addr) = address {
        let want = hypr::normalize_address(addr);
        return hypr
            .clients()
            .map_err(unreachable)?
            .into_iter()
            .find(|c| hypr::normalize_address(&c.address) == want)
            .ok_or_else(|| format!("no window with address {want}"));
    }
    if focused {
        return focused_window();
    }
    let clients = hypr.clients().map_err(unreachable)?;
    let active = hypr
        .active_window()
        .map_err(unreachable)?
        .map(|c| c.address);
    match target::owning_window(&target::ancestors(), &clients, active.as_deref()) {
        Owner::Window(c) => Ok(c.clone()),
        Owner::Ambiguous { pid, windows } => {
            let list: Vec<String> = windows
                .iter()
                .map(|c| format!("  {}  {}", c.address, c.label()))
                .collect();
            Err(format!(
                "process {pid} owns several windows and none has focus; pick one with -w:\n{}",
                list.join("\n")
            ))
        }
        Owner::None => {
            eprintln!("wname: not run from inside a window; using the focused window");
            focused_window()
        }
    }
}

fn list(hypr: &Hyprland) -> Result<()> {
    let bands = existing(hypr.signature(), &Request::List)?.unwrap_or_default();
    if bands.is_empty() {
        println!("No bands.");
        return Ok(());
    }
    let clients = hypr.clients().unwrap_or_default();
    let color_on = io::stdout().is_terminal();
    for b in &bands {
        let window = clients
            .iter()
            .find(|c| hypr::normalize_address(&c.address) == b.address)
            .map(|c| format!("{} — {}", c.class, c.label()))
            .unwrap_or_default();
        let state = b
            .hidden
            .as_deref()
            .map(|h| format!(" (hidden: {h})"))
            .unwrap_or_default();
        println!(
            "{}  {}  {:<8}  {}  [{window}]{state}",
            b.address,
            swatch(&b.color.bg, color_on),
            b.color.label,
            b.title
        );
    }
    Ok(())
}

fn swatch(bg: &str, color_on: bool) -> String {
    if !color_on {
        return "■".into();
    }
    let v = u32::from_str_radix(bg.trim_start_matches('#'), 16).unwrap_or(0);
    format!(
        "\x1b[38;2;{};{};{}m■\x1b[0m",
        v >> 16,
        (v >> 8) & 0xff,
        v & 0xff
    )
}

fn print_colors() {
    let color_on = io::stdout().is_terminal();
    for name in color::names() {
        let c = color::named(name).expect("palette name");
        println!("{}  {:<8} {}", swatch(&c.bg, color_on), name, c.bg);
    }
    println!("\nAlso: random, #rgb or #rrggbb.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> std::result::Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("wname").chain(args.iter().copied()))
    }

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn accepted_invocations() {
        for args in [
            &["-n", "Blog"][..],
            &["-n", "Blog", "-c", "red"],
            &["--name=Blog", "--color", "#abc"],
            &["-n", "Blog", "-w", "0xabc"],
            &["-n", "Blog", "--focused"],
            &["--clear", "--focused"],
            &["--clear"],
            &["--clear", "-w", "0xabc"],
            &["--clear-all"],
            &["--list"],
            &["-l"],
            &["--colors"],
            &["--quit"],
            &["--daemon"],
            &[],
        ] {
            assert!(parse(args).is_ok(), "{args:?} should parse");
        }
    }

    #[test]
    fn rejected_invocations() {
        for args in [
            &["-c", "red"][..],
            &["-n", "Blog", "--clear"],
            &["--clear", "--clear-all"],
            &["--list", "-w", "0xabc"],
            &["-n", "Blog", "--focused", "-w", "0xabc"],
            &["--list", "--focused"],
            &["--daemon", "-n", "x"],
            &["-n"],
            &["--bogus"],
        ] {
            assert!(parse(args).is_err(), "{args:?} should be rejected");
        }
    }
}
