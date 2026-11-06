use super::*;
use crate::app::{Action, App, Public, Shared, SpeedView, TargetView, View};
use crate::meta::PublicInfo;
use crate::net::{LinkKind, Local, Wifi};
use crate::ping::{Role, Summary};
use crate::speedtest::{Failure, Phase, Results};
use crate::verdict::{Cause, Diagnosis, Health};
use std::net::Ipv4Addr;
use std::time::Instant;

const PHASES: [Phase; 7] =
    [Phase::Idle, Phase::Latency, Phase::Download, Phase::Upload, Phase::Done, Phase::Cancelled, Phase::Failed];

fn summary(m: f64) -> Summary {
    Summary {
        last: Some(m),
        median: Some(m),
        jitter: Some(1.0),
        loss: Some(0.0),
        done: 60,
        replies: 60,
        lost_streak: 0,
    }
}

fn view(phase: Phase) -> View {
    let wave = |k: f64| (0..400).map(|i| (f64::from(i) * k).sin().abs() * 5e7).collect::<Vec<f64>>();
    let mut history = vec![Some(18.0); 120];
    history[100] = None;
    let done = matches!(phase, Phase::Done);
    View {
        local: Some(Local {
            name: "wlan0".into(),
            key: "wlan0".into(),
            kind: LinkKind::Wifi,
            ip: Ipv4Addr::new(192, 168, 1, 23),
            gateway: Some(Ipv4Addr::new(192, 168, 1, 1)),
            link_mbps: None,
        }),
        wifi: Some(Wifi { ssid: Some("Home".into()), dbm: Some(-58), freq_mhz: Some(5200), rate_mbps: Some(866.7) }),
        public: Public::Ready(PublicInfo {
            ip: "203.0.113.7".into(),
            isp: Some("Example Telecom".into()),
            colo: Some("BCN".into()),
            colo_city: Some("Barcelona".into()),
        }),
        rx: wave(0.37),
        tx: wave(0.11),
        now_rx: 12.4e6,
        now_tx: 0.8e6,
        peak_rx: 45e6,
        peak_tx: 3e6,
        total_rx: 120_000_000,
        total_tx: 12_000_000,
        targets: vec![
            TargetView {
                role: Role::Router,
                addr: Ipv4Addr::new(192, 168, 1, 1),
                sum: summary(2.0),
                history: vec![Some(2.0); 120],
            },
            TargetView { role: Role::Internet, addr: Ipv4Addr::new(1, 1, 1, 1), sum: summary(18.0), history },
        ],
        speed: SpeedView {
            phase,
            live: 214.0,
            progress: 0.5,
            res: Results {
                idle_ping: Some(12.0),
                idle_jitter: Some(0.5),
                down_ping: Some(27.0),
                up_ping: done.then_some(17.0),
                down: matches!(phase, Phase::Upload | Phase::Done).then_some(501.0),
                up: done.then_some(92.4),
                bytes: 1_300_000_000,
                server: Some("BCN".into()),
            },
            failure: (phase == Phase::Failed).then_some(Failure::Limited),
            finished: Some(Instant::now()),
        },
        diag: Diagnosis { health: Health::Good, cause: Cause::None },
        tcp: false,
    }
}

fn app() -> App {
    App::new(Shared::idle())
}

fn text(buf: &Buffer) -> String {
    let a = buf.area;
    (a.top()..a.bottom())
        .map(|y| (a.left()..a.right()).map(|x| buf[(x, y)].symbol().to_owned()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn draw(app: &mut App, v: &View, w: u16, h: u16) -> String {
    let area = Rect::new(0, 0, w, h);
    let mut buf = Buffer::empty(area);
    render(&mut buf, area, app, v);
    text(&buf)
}

fn cyrillic(s: &str) -> bool {
    s.chars().any(|c| ('\u{0400}'..='\u{04ff}').contains(&c))
}

#[test]
fn every_phase_renders_at_every_size() {
    let sizes = [(30, 8), (50, 14), (60, 18), (80, 24), (100, 22), (100, 30), (120, 35), (160, 48), (250, 80)];
    for (w, h) in sizes {
        for phase in PHASES {
            let s = draw(&mut app(), &view(phase), w, h);
            assert!(!s.contains('\u{2014}') && !s.contains('\u{2013}'), "long dash on screen");
            assert!(!cyrillic(&s), "{w}x{h} {phase:?}:\n{s}");
        }
    }
}

#[test]
fn page_shows_the_essentials() {
    let s = draw(&mut app(), &view(Phase::Done), 120, 32);
    for needle in [
        "traffic",
        "connection",
        "network",
        "speed test",
        "router",
        "internet",
        "1.1.1.1",
        "Home",
        "-58 dBm",
        "203.0.113.7",
        "Example Telecom",
        "Connection is healthy.",
        "▼ download",
        "▲ upload",
        "barely noticeable",
        "Barcelona (BCN)",
        "scale",
    ] {
        assert!(s.contains(needle), "{needle} is missing:\n{s}");
    }
}

#[test]
fn boxes_fill_the_whole_screen() {
    for (w, h) in [(50, 14), (80, 24), (120, 32)] {
        let s = draw(&mut app(), &view(Phase::Idle), w, h);
        let rows: Vec<&str> = s.lines().collect();
        assert!(rows[0].starts_with('╭') && rows[0].ends_with('╮'), "{w}x{h}: no box at the top:\n{s}");
        let last = rows[rows.len() - 1];
        assert!(last.starts_with('╰') && last.ends_with('╯'), "{w}x{h}: no box at the bottom:\n{s}");
    }
}

#[test]
fn english_and_on_topic() {
    assert!(!cyrillic(&crate::cli::help()));
    for phase in PHASES {
        let s = draw(&mut app(), &view(phase), 160, 48).to_lowercase();
        for word in ["game", "video", "call", "4k"] {
            assert!(!s.contains(word), "{word} on screen:\n{s}");
        }
    }
}

#[test]
fn narrow_page_keeps_the_key_facts() {
    let s = draw(&mut app(), &view(Phase::Idle), 80, 24);
    for needle in ["traffic", "connection", "router", "internet", "public ip", "Connection is healthy.", "speed test"] {
        assert!(s.contains(needle), "{needle} is missing:\n{s}");
    }
}

#[test]
fn small_window_keeps_live_traffic() {
    let s = draw(&mut app(), &view(Phase::Idle), 50, 14);
    for needle in ["traffic", "12.4", "connection", "internet"] {
        assert!(s.contains(needle), "{needle} is missing:\n{s}");
    }
}

#[test]
fn speed_test_runs_on_the_same_page() {
    let mut a = app();
    let s = draw(&mut a, &view(Phase::Download), 120, 32);
    for needle in ["traffic", "connection", "network", "▼ download", "esc stop", "50%"] {
        assert!(s.contains(needle), "{needle} is missing:\n{s}");
    }
    assert!(a.hits.iter().any(|(_, act)| *act == Action::Cancel), "esc is not clickable");
    assert!(!a.hits.iter().any(|(_, act)| *act == Action::Speed), "a second test can be started");
}

#[test]
fn border_keys_are_clickable() {
    let mut a = app();
    let s = draw(&mut a, &view(Phase::Idle), 120, 32);
    let rows: Vec<&str> = s.lines().collect();
    for act in [Action::Speed, Action::Quit] {
        let (r, _) = a.hits.iter().find(|(r, x)| *x == act && rows[usize::from(r.y)].contains('╯')).expect("hint");
        let row: String = rows[usize::from(r.y)].chars().skip(usize::from(r.x)).take(usize::from(r.width)).collect();
        assert!(row.starts_with(if act == Action::Speed { "s start" } else { "q quit" }), "{row}");
    }
}

#[test]
fn results_survive_short_boxes() {
    for (w, h) in [(80, 24), (100, 22), (120, 32), (160, 48)] {
        let s = draw(&mut app(), &view(Phase::Done), w, h);
        assert!(s.contains(" A  barely noticeable") && s.contains("ping"), "{w}x{h}:\n{s}");
    }
}

#[test]
fn failure_is_explained() {
    let mut a = app();
    let s = draw(&mut a, &view(Phase::Failed), 120, 32);
    assert!(s.contains("Cloudflare limits"), "{s}");
    assert!(a.hits.iter().any(|(_, act)| *act == Action::Speed), "no way to start again");
}

#[test]
fn long_verdict_takes_two_lines() {
    let mut v = view(Phase::Idle);
    v.diag = Diagnosis { health: Health::Down, cause: Cause::NoInternet };
    let s = draw(&mut app(), &v, 80, 24);
    let rows: Vec<&str> = s.lines().collect();
    let i = rows.iter().position(|r| r.contains("No internet:")).expect("verdict");
    assert!(rows[i + 1].contains("line is down."), "{s}");
}

#[test]
fn tiny_window_asks_for_more_room() {
    let s = draw(&mut app(), &view(Phase::Idle), 30, 8);
    assert!(s.contains("Make the window"));
}
