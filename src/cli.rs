// Command line options and the speed test without the interface.

use crate::fmt;
use crate::speedtest::{Phase, Results, SpeedTest};
use crate::util::lock;
use std::ffi::OsString;
use std::io::{IsTerminal, Write};
use std::time::Duration;

const HELP: &str = "netekg {version}: vital signs of your internet connection

Usage:
  netekg                  open the dashboard
  netekg --speed          run a speed test and print the result
  netekg --speed --json   the same as JSON

Options:
  -s, --speed       speed test without the interface
      --json        machine-readable output for --speed
      --tcp         time TCP handshakes instead of ICMP pings
  -h, --help        show this help
  -V, --version     show the version

Keys in the dashboard: s starts the speed test, esc stops it, q quits.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Tui,
    Speed { json: bool },
    Help,
    Version,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opts {
    pub mode: Mode,
    pub tcp: bool,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Opts, String> {
    let mut opts = Opts { mode: Mode::Tui, tcp: false };
    let (mut speed, mut json) = (false, false);
    for arg in args {
        match arg.to_string_lossy().as_ref() {
            "-s" | "--speed" => speed = true,
            "--json" => json = true,
            "--tcp" => opts.tcp = true,
            "-h" | "--help" => opts.mode = Mode::Help,
            "-V" | "--version" => opts.mode = Mode::Version,
            other => return Err(format!("unknown option: {other}\nnetekg --help")),
        }
    }
    if opts.mode == Mode::Tui && (speed || json) {
        opts.mode = Mode::Speed { json };
    }
    Ok(opts)
}

pub fn help() -> String {
    HELP.replace("{version}", env!("CARGO_PKG_VERSION"))
}

pub fn run_speed(json: bool) -> i32 {
    let test = SpeedTest::new();
    test.start();
    let live = !json && std::io::stderr().is_terminal();
    let mut err = std::io::stderr();
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let s = lock(&test.state);
        if live {
            let _ = write!(err, "\r\x1b[2K{}", progress_line(s.phase, s.live, s.progress()));
            let _ = err.flush();
        }
        if !s.running() {
            break;
        }
    }
    if live {
        let _ = write!(err, "\r\x1b[2K");
    }
    let s = lock(&test.state);
    match s.phase {
        Phase::Done if json => {
            let r = &s.res;
            let v = serde_json::json!({
                "server": r.server,
                "ping_ms": r.idle_ping,
                "jitter_ms": r.idle_jitter,
                "download_mbps": r.down,
                "upload_mbps": r.up,
                "download_loaded_ping_ms": r.down_ping,
                "upload_loaded_ping_ms": r.up_ping,
                "lag_grade": r.grade().map(|g| g.label()),
                "data_bytes": r.bytes,
            });
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            0
        }
        Phase::Done => {
            print!("{}", report(&s.res));
            0
        }
        Phase::Cancelled => 130,
        _ => {
            let why = s.failure.map_or("unknown error", |f| f.describe());
            eprintln!("netekg: the test failed: {why}");
            1
        }
    }
}

fn progress_line(phase: Phase, live: f64, progress: f64) -> String {
    let width = 24;
    let filled = ((progress.clamp(0.0, 1.0) * width as f64).round() as usize).min(width);
    let bar = format!("{}{}", "#".repeat(filled), "-".repeat(width - filled));
    let what = match phase {
        Phase::Latency => "measuring ping".to_owned(),
        Phase::Download => format!("download {}", fmt::mbit(live)),
        Phase::Upload => format!("upload {}", fmt::mbit(live)),
        _ => String::new(),
    };
    format!("[{bar}] {what}")
}

pub fn report(r: &Results) -> String {
    let ms = |v: Option<f64>| v.map_or_else(|| "?".to_owned(), fmt::ms);
    let mbit = |v: Option<f64>| v.map_or_else(|| "?".to_owned(), fmt::mbit);
    let lag = match (r.grade(), r.lag_increase()) {
        (Some(g), Some(inc)) => format!("{}: {} (+{})", g.label(), g.describe(), fmt::ms(inc)),
        _ => "?".to_owned(),
    };
    let rows = [
        ("Server", format!("{} (Cloudflare)", r.server.as_deref().unwrap_or("?"))),
        ("Ping", format!("{}  (jitter {})", ms(r.idle_ping), ms(r.idle_jitter))),
        ("Download", format!("{}  (under load {})", mbit(r.down), ms(r.down_ping))),
        ("Upload", format!("{}  (under load {})", mbit(r.up), ms(r.up_ping))),
        ("Lag under load", lag),
        ("Data used", fmt::bytes(r.bytes as f64)),
    ];
    let w = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0) + 2;
    rows.iter().map(|(k, v)| format!("{k:<w$}{v}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Opts, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn modes() {
        assert_eq!(p(&[]).unwrap().mode, Mode::Tui);
        assert_eq!(p(&["--speed"]).unwrap().mode, Mode::Speed { json: false });
        assert_eq!(p(&["-s", "--json"]).unwrap().mode, Mode::Speed { json: true });
        assert_eq!(p(&["--help"]).unwrap().mode, Mode::Help);
        assert_eq!(p(&["-V"]).unwrap().mode, Mode::Version);
        assert!(p(&["--tcp"]).unwrap().tcp);
        assert!(p(&["--bogus"]).is_err());
    }

    #[test]
    fn report_has_every_line() {
        let r = Results {
            idle_ping: Some(18.0),
            idle_jitter: Some(1.2),
            down_ping: Some(40.0),
            up_ping: Some(30.0),
            down: Some(314.0),
            up: Some(92.4),
            bytes: 1_300_000_000,
            server: Some("BCN".into()),
        };
        let text = report(&r);
        assert!(text.contains("314 Mbps"));
        assert!(text.contains("92.4 Mbps"));
        assert!(text.contains("A: barely noticeable (+22 ms)"));
        assert!(text.contains("1.30 GB"));
        assert_eq!(text.lines().count(), 6);
    }
}
