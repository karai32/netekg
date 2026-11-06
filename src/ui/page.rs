// The page: traffic, connection, network and speed test boxes.

use super::theme::{
    BAD, BG, BRIGHT, CONNECTION, DIM, DOWN, DOWN_RAMP, FAINT, GOOD, HEAT, NETWORK, Rgb, SPEED, TEXT, TRACK, TRAFFIC,
    UP, UP_RAMP, WARN, mix, ramp,
};
use super::widgets::{
    Border, Piece, Size, badge, bar, bars_down, bars_up, big, big_width, bold, boxed, nice_ceil, put, set, spans,
    spark, spinner, st, truncate, width, wrap,
};
use crate::app::{Action, App, Public, TargetView, View};
use crate::fmt;
use crate::net::LinkKind;
use crate::ping::Role;
use crate::speedtest::{Grade, LATENCY, PHASE, Phase};
use crate::verdict::{Cause, Health};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;

fn p(s: impl Into<String>, style: Style) -> Piece {
    (s.into(), style)
}

pub fn draw(buf: &mut Buffer, area: Rect, app: &mut App, v: &View) {
    // Traffic and connection always stay; network goes first.
    const TRAFFIC_MIN: u16 = 4;
    let spin = spinner(app.epoch.elapsed().as_millis());
    let lines = network_lines(v);
    let verdict = verdict_lines(diagnosis(v), usize::from(area.width.saturating_sub(6)));
    let rows = v.targets.len().max(2) as u16;
    let conn_h = 3 + verdict.len() as u16 + rows;
    if area.width >= 100 {
        let conn_h = conn_h.min(area.height.saturating_sub(10));
        let [top, conn_a] = Layout::vertical([Constraint::Fill(1), Constraint::Length(conn_h)]).areas(area);
        let right_w = (area.width * 2 / 5).clamp(40, 58);
        let [traffic_a, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_w)]).areas(top);
        let net_h = (lines.len() as u16 + 2).min(right.height.saturating_sub(8));
        let [net_a, speed_a] = Layout::vertical([Constraint::Length(net_h), Constraint::Fill(1)]).areas(right);
        traffic(buf, traffic_a, v);
        connection(buf, conn_a, v, &verdict, spin);
        network(buf, net_a, lines);
        speed(buf, speed_a, v, spin, &mut app.hits);
        return;
    }
    let busy = v.speed.phase != Phase::Idle;
    let mut bottom_h = (area.height.saturating_sub(conn_h) / 2).clamp(6, 10);
    if TRAFFIC_MIN + conn_h + bottom_h > area.height {
        bottom_h = area.height.saturating_sub(TRAFFIC_MIN + conn_h);
        // A started test keeps at least its numbers on screen.
        if bottom_h < if busy { 3 } else { 4 } {
            bottom_h = 0;
        }
    }
    let conn_h = conn_h.min(area.height.saturating_sub(TRAFFIC_MIN + bottom_h));
    let [traffic_a, conn_a, bottom] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(conn_h), Constraint::Length(bottom_h)]).areas(area);
    traffic(buf, traffic_a, v);
    connection(buf, conn_a, v, &verdict, spin);
    if busy && bottom_h < 6 {
        speed(buf, bottom, v, spin, &mut app.hits);
    } else {
        let [net_a, speed_a] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(bottom);
        network(buf, net_a, lines);
        speed(buf, speed_a, v, spin, &mut app.hits);
    }
}

// Wrapped text from row `y`; returns the next row.
fn text(buf: &mut Buffer, inner: Rect, y: u16, s: &str, style: Style, indent: u16) -> u16 {
    let mut y = y;
    for line in wrap(s, usize::from(inner.width.saturating_sub(indent))) {
        if y >= inner.bottom() {
            break;
        }
        put(buf, inner.x + indent, y, &line, style, inner.width.saturating_sub(indent));
        y += 1;
    }
    y
}

fn traffic(buf: &mut Buffer, area: Rect, v: &View) {
    let n = usize::from(area.width.saturating_sub(4));
    let rx = &v.rx[v.rx.len().saturating_sub(n)..];
    let tx = &v.tx[v.tx.len().saturating_sub(n)..];
    let dmax = nice_ceil(rx.iter().copied().fold(0.0, f64::max).max(1e5));
    let umax = nice_ceil(tx.iter().copied().fold(0.0, f64::max).max(1e5));
    let chart = v.local.is_some() && area.height >= 6;
    let mut border = Border::default();
    if let Some(l) = &v.local {
        border.status = vec![p(&l.name, st(DIM))];
    }
    // Chart scale in the bottom border.
    if chart {
        border.keys = vec![
            p("scale ", st(DIM)),
            p("▼ ", st(DOWN)),
            p(fmt::bits_short(dmax), st(TEXT)),
            p("  ▲ ", st(UP)),
            p(fmt::bits_short(umax), st(TEXT)),
        ];
    }
    let inner = boxed(buf, area, &TRAFFIC, "traffic", &border);
    if inner.height == 0 {
        return;
    }
    if v.local.is_none() {
        put(buf, inner.x, inner.y, "No network connection", bold(BAD), inner.width);
        return;
    }
    stats(buf, inner, inner.y, ("▼", DOWN), v.now_rx, v.peak_rx, v.total_rx);
    if inner.height >= 2 {
        stats(buf, inner, inner.bottom() - 1, ("▲", UP), v.now_tx, v.peak_tx, v.total_tx);
    }
    if chart {
        let area = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 2);
        mirror(buf, area, rx.iter().map(|x| x / dmax).collect(), tx.iter().map(|x| x / umax).collect());
    }
}

fn stats(buf: &mut Buffer, inner: Rect, y: u16, (arrow, color): (&str, Rgb), now: f64, peak: f64, total: u64) {
    let (num, unit) = fmt::bits_parts(now);
    let left = [
        p(arrow, bold(color)),
        p(" ", st(TEXT)),
        p(num, bold(BRIGHT)),
        p(format!(" {unit:<6}"), st(TEXT)),
        p(fmt::byte_rate(now), st(DIM)),
    ];
    let x = spans(buf, inner.x, y, &left, inner.right());
    let side = [
        p("peak ", st(DIM)),
        p(fmt::bits(peak), st(TEXT)),
        p("   total ", st(DIM)),
        p(fmt::bytes(total as f64), st(TEXT)),
    ];
    let w: u16 = side.iter().map(|(s, _)| width(s)).sum();
    let sx = inner.right().saturating_sub(w);
    if sx > x + 2 {
        spans(buf, sx, y, &side, inner.right());
    }
}

// Download grows up from the middle, upload hangs down.
fn mirror(buf: &mut Buffer, area: Rect, down: Vec<f64>, up: Vec<f64>) {
    let top_h = area.height - area.height / 2;
    let bot_h = area.height / 2;
    for x in area.left()..area.right() {
        set(buf, x, area.y + top_h - 1, "▁", st(FAINT));
    }
    let n = area.width;
    let (dn, un) = (down.len() as u16, up.len() as u16);
    bars_up(buf, Rect::new(area.x + n - dn, area.y, dn, top_h), &down, &DOWN_RAMP);
    bars_down(buf, Rect::new(area.x + n - un, area.y + top_h, un, bot_h), &up, &UP_RAMP);
}

fn diagnosis(v: &View) -> &'static str {
    match (v.diag.health, v.diag.cause) {
        (Health::Measuring, _) => "Measuring the connection...",
        (_, Cause::NoNetwork) => "No network: this computer is not connected to anything.",
        (_, Cause::NoRouter) => "The router doesn't answer: check the Wi-Fi or the cable.",
        (_, Cause::NoInternet) => {
            "No internet: the router answers, but nothing beyond it does. The provider or the router's line is down."
        }
        (_, Cause::Down) => "No internet connection.",
        (_, Cause::Local) => "Trouble between you and the router: Wi-Fi or cable.",
        (_, Cause::WeakWifi) => "Weak Wi-Fi signal: move closer to the router or use a cable.",
        (_, Cause::Beyond) => "Home network is fine, the trouble is further out: at the provider or beyond.",
        (_, Cause::Unstable) => "The connection is unstable.",
        (_, Cause::None) => "Connection is healthy.",
    }
}

// At most two lines, the rest cut with an ellipsis.
fn verdict_lines(text: &str, w: usize) -> Vec<String> {
    let mut lines = wrap(text, w.max(1));
    if lines.len() > 2 {
        let rest = lines.split_off(1).join(" ");
        lines.push(truncate(&rest, w));
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn health_color(h: Health) -> Rgb {
    match h {
        Health::Good => GOOD,
        Health::Fair => WARN,
        Health::Poor | Health::Down => BAD,
        Health::Measuring => DIM,
    }
}

// Green when fine, through yellow to red when bad.
fn heat(role: Role, ms: f64) -> Rgb {
    let (good, bad) = if role == Role::Router { (10.0, 30.0) } else { (60.0, 120.0) };
    ramp(&HEAT, (ms - good) / (bad - good))
}

fn silent_router(tv: &TargetView) -> bool {
    tv.role == Role::Router && tv.sum.replies == 0 && tv.sum.done >= 5
}

fn status_color(tv: &TargetView) -> Rgb {
    let s = &tv.sum;
    if s.done == 0 || silent_router(tv) {
        return FAINT;
    }
    if s.replies == 0 || s.lost_streak >= 3 || s.loss_or_zero() >= 2.0 {
        return BAD;
    }
    if s.loss_or_zero() > 0.0 { WARN } else { heat(tv.role, s.median.unwrap_or(0.0)) }
}

fn connection(buf: &mut Buffer, area: Rect, v: &View, verdict: &[String], spin: &str) {
    let mut border = Border::default();
    if v.tcp {
        border.note = vec![p("tcp pings: icmp is blocked here", st(DIM))];
    }
    let inner = boxed(buf, area, &CONNECTION, "connection", &border);
    if inner.height == 0 {
        return;
    }
    let color = health_color(v.diag.health);
    let mark = if v.diag.health == Health::Measuring { spin } else { "●" };
    let mut y = inner.y;
    for (i, line) in verdict.iter().enumerate() {
        if y >= inner.bottom() {
            return;
        }
        if i == 0 {
            put(buf, inner.x, y, mark, bold(color), 1);
        }
        put(buf, inner.x + 2, y, line, bold(color), inner.width.saturating_sub(2));
        y += 1;
    }
    let has_router = v.targets.iter().any(|tv| tv.role == Role::Router);
    let missing_router = !has_router && v.local.is_some();
    let rows = v.targets.len() as u16 + u16::from(missing_router);
    let name_w: u16 = 12;
    let ip_w: u16 = if inner.width >= 74 { 16 } else { 0 };
    let num_w: u16 = 10;
    let x0 = inner.x + name_w + ip_w;
    let hist_x = x0 + num_w * 3;
    let right = inner.right();
    // Column names only when all rows still fit.
    if inner.bottom().saturating_sub(y) > rows {
        for (i, label) in ["ping", "jitter", "loss"].iter().enumerate() {
            put(buf, x0 + num_w * i as u16, y, label, st(DIM), num_w);
        }
        if hist_x + 8 < right {
            put(buf, hist_x, y, "history", st(DIM), right - hist_x);
        }
        y += 1;
    }
    if missing_router && y < inner.bottom() {
        spans(buf, inner.x, y, &[p("●", st(FAINT)), p(" router", st(BRIGHT))], x0);
        put(buf, x0, y, "not found", st(DIM), right.saturating_sub(x0));
        y += 1;
    }
    for tv in &v.targets {
        if y >= inner.bottom() {
            break;
        }
        let name = if tv.role == Role::Router { " router" } else { " internet" };
        spans(buf, inner.x, y, &[p("●", st(status_color(tv))), p(name, st(BRIGHT))], x0);
        if ip_w > 0 {
            put(buf, inner.x + name_w, y, &tv.addr.to_string(), st(DIM), ip_w);
        }
        let s = &tv.sum;
        if silent_router(tv) {
            put(buf, x0, y, "doesn't answer pings", st(DIM), right.saturating_sub(x0));
        } else if s.done == 0 || s.replies == 0 && s.done < 3 {
            put(buf, x0, y, "...", st(DIM), num_w);
        } else {
            let loss = s.loss_or_zero();
            let loss_c = if loss >= 2.0 {
                BAD
            } else if loss > 0.0 {
                WARN
            } else {
                TEXT
            };
            let ping = s.median.map_or(("?".into(), DIM), |m| (fmt::ms(m), heat(tv.role, m)));
            put(buf, x0, y, &ping.0, bold(ping.1), num_w);
            put(buf, x0 + num_w, y, &s.jitter.map_or("?".into(), fmt::ms), st(TEXT), num_w);
            put(buf, x0 + num_w * 2, y, &fmt::pct(loss), st(loss_c), num_w);
            if hist_x + 8 < right {
                history(buf, hist_x, y, right - hist_x, tv);
            }
        }
        y += 1;
    }
}

fn history(buf: &mut Buffer, x: u16, y: u16, w: u16, tv: &TargetView) {
    let h = &tv.history[tv.history.len().saturating_sub(usize::from(w))..];
    let mut vals: Vec<f64> = h.iter().flatten().copied().collect();
    vals.sort_by(f64::total_cmp);
    let p90 = vals.get(vals.len() * 9 / 10).copied().unwrap_or(0.0);
    let floor = if tv.role == Role::Router { 10.0 } else { 40.0 };
    let scale = (p90 * 1.5).max(floor);
    let off = w - h.len() as u16;
    for i in 0..off {
        set(buf, x + i, y, "▁", st(TRACK));
    }
    for (i, s) in h.iter().enumerate() {
        let (sym, color) = match s {
            Some(ms) => (spark(ms / scale), heat(tv.role, *ms)),
            None => ("×", BAD),
        };
        set(buf, x + off + i as u16, y, sym, st(color));
    }
}

struct Row {
    label: &'static str,
    value: Value,
    // Lowest priority goes first when space runs out.
    prio: u8,
}

enum Value {
    Text(Vec<Piece>),
    // dBm and level.
    Signal(i32, usize),
}

fn text_row(label: &'static str, parts: Vec<Piece>, prio: u8) -> Row {
    Row { label, value: Value::Text(parts), prio }
}

fn network_lines(v: &View) -> Vec<Row> {
    let Some(l) = &v.local else {
        return vec![text_row("network", vec![p("not connected", bold(BAD))], 9)];
    };
    let kind = match l.kind {
        LinkKind::Wifi => "wi-fi",
        LinkKind::Ethernet => "ethernet",
        LinkKind::Vpn => "vpn",
        LinkKind::Mobile => "mobile",
        LinkKind::Other => "network",
    };
    let main = match v.wifi.as_ref().and_then(|w| w.ssid.clone()) {
        Some(ssid) => ssid,
        None => match l.link_mbps {
            Some(m) => format!("{} · {}", l.name, fmt::mbit(m as f64)),
            None => l.name.clone(),
        },
    };
    let mut rows = vec![text_row(kind, vec![p(main, bold(BRIGHT))], 9)];
    if let Some(w) = &v.wifi {
        if let (Some(dbm), Some(level)) = (w.dbm, w.level()) {
            rows.push(Row { label: "signal", value: Value::Signal(dbm, level), prio: 6 });
        }
        let mut radio = Vec::new();
        if let Some(b) = w.band_ghz() {
            radio.push(format!("{} GHz", fmt::num(b, if b < 3.0 { 1 } else { 0 })));
        }
        if let Some(c) = w.channel() {
            radio.push(format!("ch {c}"));
        }
        if let Some(r) = w.rate_mbps {
            radio.push(fmt::mbit(r));
        }
        if !radio.is_empty() {
            rows.push(text_row("radio", vec![p(radio.join(" · "), st(TEXT))], 1));
        }
    }
    rows.push(text_row("local ip", vec![p(l.ip.to_string(), st(TEXT))], 2));
    let gw = l.gateway.map_or_else(|| "not found".to_owned(), |g| g.to_string());
    rows.push(text_row("router", vec![p(gw, st(TEXT))], 3));
    let (ip, isp) = match &v.public {
        Public::Ready(info) => {
            (p(&info.ip, bold(BRIGHT)), p(info.isp.clone().unwrap_or_else(|| "unknown".into()), st(TEXT)))
        }
        Public::Loading => (p("loading...", st(DIM)), p("loading...", st(DIM))),
        Public::Failed => (p("unavailable", st(DIM)), p("unavailable", st(DIM))),
    };
    rows.push(text_row("public ip", vec![ip], 8));
    rows.push(text_row("provider", vec![isp], 7));
    rows
}

// Signal gauge from -90 to -40 dBm; it shrinks to fit, then the word goes.
fn signal(dbm: i32, level: usize, room: u16) -> Vec<Piece> {
    let color = [GOOD, GOOD, WARN, BAD][level];
    let reading = format!("  {dbm} dBm ");
    let word = ["excellent", "good", "fair", "weak"][level];
    let spare = room.saturating_sub(width(&reading) + width(word));
    let (segments, word) =
        if spare >= 4 { (spare.min(10), word) } else { (room.saturating_sub(width(&reading)).min(10), "") };
    let share = (f64::from(dbm) + 90.0) / 50.0;
    let filled = (share * f64::from(segments)).round().clamp(1.0, f64::from(segments.max(1))) as u16;
    let mut parts: Vec<Piece> = (0..segments)
        .map(|i| {
            let shade = mix(mix(color, BG, 0.55), color, f64::from(i) / f64::from(segments.max(2) - 1));
            p("■", st(if i < filled { shade } else { FAINT }))
        })
        .collect();
    parts.push(p(reading, st(TEXT)));
    if !word.is_empty() {
        parts.push(p(word, st(color)));
    }
    parts
}

fn network(buf: &mut Buffer, area: Rect, mut rows: Vec<Row>) {
    let inner = boxed(buf, area, &NETWORK, "network", &Border::default());
    while rows.len() > usize::from(inner.height) && !rows.is_empty() {
        let i = rows.iter().enumerate().min_by_key(|(_, r)| r.prio).map_or(0, |(i, _)| i);
        rows.remove(i);
    }
    let label_w = rows.iter().map(|r| width(r.label)).max().unwrap_or(0) + 2;
    for (i, r) in rows.iter().enumerate() {
        let y = inner.y + i as u16;
        put(buf, inner.x, y, r.label, st(DIM), label_w);
        let mut x = inner.x + label_w;
        let parts = match &r.value {
            Value::Text(parts) => parts.clone(),
            Value::Signal(dbm, level) => signal(*dbm, *level, inner.right().saturating_sub(x)),
        };
        for (s, style) in parts {
            let room = inner.right().saturating_sub(x);
            x = put(buf, x, y, &truncate(&s, usize::from(room)), style, room);
        }
    }
}

fn grade_color(g: Grade) -> Rgb {
    match g {
        Grade::APlus | Grade::A | Grade::B => GOOD,
        Grade::C => WARN,
        Grade::D | Grade::F => BAD,
    }
}

// Key hints for the bottom border, with matching click targets.
fn keys(area: Rect, title: &str, keys: &[(&str, &str, Action)], hits: &mut Vec<(Rect, Action)>) -> Vec<Piece> {
    let mut pieces = Vec::new();
    for (i, (key, what, _)) in keys.iter().enumerate() {
        if i > 0 {
            pieces.push(p("  ", st(DIM)));
        }
        pieces.push(p(*key, bold(SPEED.title)));
        pieces.push(p(format!(" {what}"), st(DIM)));
    }
    let w: u16 = pieces.iter().map(|(s, _)| width(s)).sum();
    let r = area.right().saturating_sub(1);
    if area.height >= 2 && r >= area.x + 2 + width(title) + 2 + w + 4 {
        let mut x = r - 3 - w + 1;
        for (key, what, action) in keys {
            let kw = width(key) + 1 + width(what);
            hits.push((Rect::new(x, area.bottom() - 1, kw, 1), *action));
            x += kw + 2;
        }
    }
    pieces
}

fn phase_look(phase: Phase) -> (&'static str, Rgb) {
    match phase {
        Phase::Download => ("download", DOWN),
        Phase::Upload => ("upload", UP),
        _ => ("ping", SPEED.title),
    }
}

fn speed(buf: &mut Buffer, area: Rect, v: &View, spin: &str, hits: &mut Vec<(Rect, Action)>) {
    const TITLE: &str = "speed test";
    let s = &v.speed;
    let running = s.running();
    let hints: &[(&str, &str, Action)] = match s.phase {
        _ if running => &[("esc", "stop", Action::Cancel), ("q", "quit", Action::Quit)],
        Phase::Idle => &[("s", "start", Action::Speed), ("q", "quit", Action::Quit)],
        _ => &[("s", "again", Action::Speed), ("q", "quit", Action::Quit)],
    };
    let status = match s.phase {
        _ if running => {
            let (name, color) = phase_look(s.phase);
            let pct = format!("{:.0}%", s.progress * 100.0);
            vec![p(spin, bold(color)), p(format!(" {name} "), st(TEXT)), p(pct, st(DIM))]
        }
        Phase::Done => {
            let ago = s.finished.map(|f| fmt::ago(f.elapsed().as_secs())).unwrap_or_default();
            vec![p("✓ ", bold(GOOD)), p(ago, st(DIM))]
        }
        Phase::Cancelled => vec![p("stopped", st(WARN))],
        Phase::Failed => vec![p("failed", st(BAD))],
        _ => Vec::new(),
    };
    let border = Border { status, note: Vec::new(), keys: keys(area, TITLE, hints, hits) };
    let inner = boxed(buf, area, &SPEED, TITLE, &border);
    if inner.height == 0 {
        return;
    }
    match s.phase {
        Phase::Idle => {
            let mut y = start_line(buf, inner, inner.y, " to start", hits) + 1;
            if inner.height >= 8 {
                let lag = "lag under load: how much the ping grows while the line is busy";
                for (mark, color, what) in
                    [("▼", DOWN, "download speed"), ("▲", UP, "upload speed"), ("◆", SPEED.title, lag)]
                {
                    if y >= inner.bottom() {
                        break;
                    }
                    put(buf, inner.x, y, mark, bold(color), 1);
                    y = text(buf, inner, y, what, st(TEXT), 2);
                }
                y += 1;
            }
            text(
                buf,
                inner,
                y,
                "About 25 seconds at full speed: on a metered connection it uses a lot of data.",
                st(DIM),
                0,
            );
        }
        Phase::Done => results(buf, inner, v),
        Phase::Cancelled => {
            let y = text(buf, inner, inner.y, "The test was stopped.", st(WARN), 0);
            start_line(buf, inner, y + 1, " to run it again", hits);
        }
        Phase::Failed => {
            let why = s.failure.map_or("unknown error", |f| f.describe());
            let y = text(buf, inner, inner.y, &format!("The test failed: {why}."), st(BAD), 0);
            start_line(buf, inner, y + 1, " to try again", hits);
        }
        _ => progress(buf, inner, v),
    }
}

// A clickable "▶ press s ..." line that starts the test.
fn start_line(buf: &mut Buffer, inner: Rect, y: u16, tail: &str, hits: &mut Vec<(Rect, Action)>) -> u16 {
    if y >= inner.bottom() {
        return y;
    }
    let parts = [p("▶ ", bold(SPEED.title)), p("press ", st(TEXT)), p("s", bold(SPEED.title)), p(tail, st(TEXT))];
    let end = spans(buf, inner.x, y, &parts, inner.right());
    hits.push((Rect::new(inner.x, y, end - inner.x, 1), Action::Speed));
    y + 1
}

// How the download and upload numbers are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Numbers {
    // Side by side; true puts the unit on its own row.
    Big(Size, bool),
    // One above the other, for tall and narrow boxes.
    Tall(Size),
    // Both on one line.
    Side,
    // One line each.
    Stacked,
}

impl Numbers {
    fn height(self) -> u16 {
        match self {
            Numbers::Big(size, below) => 1 + size.height() + u16::from(below),
            Numbers::Tall(size) => 3 + size.height() * 2,
            Numbers::Side => 1,
            Numbers::Stacked => 2,
        }
    }
}

// Download and upload in Mbit/s, live or final.
fn shown(v: &View) -> [Option<f64>; 2] {
    let s = &v.speed;
    let live = |p: Phase| (s.phase == p).then_some(s.live);
    [live(Phase::Download).or(s.res.down), live(Phase::Upload).or(s.res.up)]
}

// Biggest digits that leave `rest` rows free, else plain text.
fn numbers_mode(inner: Rect, rest: u16) -> Numbers {
    let half = (inner.width / 2).saturating_sub(1);
    let room = inner.height.saturating_sub(rest);
    let unit = width("Mbps") + 1;
    // Widest possible number, so the size doesn't jump.
    let digits = |size| big_width("88.8", size);
    let side = |size: Size| {
        let below = digits(size) + unit > half;
        (digits(size) <= half && 1 + size.height() + u16::from(below) <= room).then_some(Numbers::Big(size, below))
    };
    let tall = (digits(Size::Large) + unit <= inner.width).then_some(Numbers::Tall(Size::Large));
    side(Size::Large)
        .or(tall.filter(|m| m.height() <= room))
        .or_else(|| side(Size::Small))
        .unwrap_or(if inner.width / 2 >= 15 { Numbers::Side } else { Numbers::Stacked })
}

fn numbers(buf: &mut Buffer, inner: Rect, y: u16, v: &View, mode: Numbers) -> u16 {
    let half = inner.width / 2;
    let cols = [("▼", "download", &DOWN_RAMP, Phase::Download), ("▲", "upload", &UP_RAMP, Phase::Upload)];
    let values = shown(v).map(|x| x.map(|m| fmt::bits_parts(m * 1e6)));
    // Stacked lines get names when they fit.
    let name_w = width("download") + 4;
    let value_w = values.iter().flatten().map(|(n, u)| width(n) + 1 + width(u)).max().unwrap_or(1);
    let named = name_w + value_w <= inner.width;
    for (i, ((arrow, name, stops, phase), value)) in cols.into_iter().zip(values).enumerate() {
        let (num, unit) = value.as_ref().map_or(("-", ""), |(n, u)| (n.as_str(), *u));
        let color = stops[1];
        let label = format!("{arrow} {name}");
        let label_style = if v.speed.phase == phase { bold(color) } else { st(color) };
        let shade = (stops[2], stops[1]);
        let i = i as u16;
        match mode {
            Numbers::Big(size, _) | Numbers::Tall(size) => {
                let (x, top, w) = match mode {
                    Numbers::Tall(_) => (inner.x, y + i * (size.height() + 2), inner.width),
                    _ => (inner.x + half * i, y, half.saturating_sub(1)),
                };
                put(buf, x, top, &label, label_style, w);
                if value.is_none() {
                    put(buf, x, top + 1 + size.height() / 2, "waiting", st(FAINT), w);
                    continue;
                }
                big(buf, x, top + 1, num, shade, size);
                let below = matches!(mode, Numbers::Big(_, true));
                let (ux, uy) = if below {
                    (x, top + 1 + size.height())
                } else {
                    (x + big_width(num, size) + 1, top + size.height())
                };
                put(buf, ux, uy, unit, st(DIM), w);
            }
            Numbers::Side | Numbers::Stacked => {
                let (x, row, w) = if mode == Numbers::Side {
                    (inner.x + half * i, y, half.saturating_sub(1))
                } else {
                    (inner.x, y + i, inner.width)
                };
                if row >= inner.bottom() {
                    continue;
                }
                let head = if mode == Numbers::Stacked && named {
                    format!("{label:<0$}", usize::from(name_w))
                } else {
                    format!("{arrow} ")
                };
                let num_style = if value.is_some() { bold(BRIGHT) } else { st(FAINT) };
                let parts = [p(head, label_style), p(num, num_style), p(" ", st(DIM)), p(unit, st(DIM))];
                spans(buf, x, row, &parts, x + w);
            }
        }
    }
    y + mode.height()
}

fn progress(buf: &mut Buffer, inner: Rect, v: &View) {
    let s = &v.speed;
    let mode = numbers_mode(inner, 5);
    let mut y = numbers(buf, inner, inner.y, v, mode);
    let roomy = inner.height >= mode.height() + 4;
    y += u16::from(roomy);
    if y < inner.bottom() {
        // One bar for the whole test, colored by phase.
        let total = (LATENCY + PHASE * 2).as_secs_f64();
        let (ping_end, down_end) = (LATENCY.as_secs_f64() / total, (LATENCY + PHASE).as_secs_f64() / total);
        let color = |f: f64| {
            if f < ping_end {
                SPEED.title
            } else if f < down_end {
                ramp(&DOWN_RAMP[..2], (f - ping_end) / (down_end - ping_end))
            } else {
                ramp(&UP_RAMP[..2], (f - down_end) / (1.0 - down_end))
            }
        };
        bar(buf, inner.x, y, inner.width, s.progress, color);
        y += 1;
    }
    y += u16::from(roomy);
    if let (Some(ping), true) = (s.res.idle_ping, y < inner.bottom()) {
        let parts = [p("ping ", st(DIM)), p(fmt::ms(ping), st(TEXT)), p(" idle", st(DIM))];
        spans(buf, inner.x, y, &parts, inner.right());
    }
}

enum Item {
    Gap,
    Line(&'static str, Vec<Piece>),
}

fn results(buf: &mut Buffer, inner: Rect, v: &View) {
    let r = &v.speed.res;
    let loaded = match (r.down_ping, r.up_ping) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    let ping = match (r.idle_ping, loaded) {
        (Some(i), Some(l)) => format!("{} idle · {} loaded", fmt::ms(i), fmt::ms(l)),
        (Some(i), None) => format!("{} idle", fmt::ms(i)),
        _ => "?".into(),
    };
    // The number is the priority.
    let mut items: Vec<(Item, u8)> = vec![(Item::Gap, 1), (Item::Line("ping", vec![p(ping, st(TEXT))]), 6)];
    if let Some(g) = r.grade() {
        let lag = vec![p(format!(" {} ", g.label()), badge(grade_color(g))), p(format!(" {}", g.describe()), st(TEXT))];
        items.push((Item::Line("lag", lag), 8));
    }
    if let Some(name) = super::server_name(v) {
        items.push((Item::Line("server", vec![p(name, st(TEXT))]), 3));
    }
    items.push((Item::Line("data used", vec![p(fmt::bytes(r.bytes as f64), st(TEXT))]), 4));

    let mode = numbers_mode(inner, 5);
    let label_w = width("data used") + 2;
    // Value column: aligned, or right after the label; None means the next row.
    let value_x = |it: &Item| -> Option<u16> {
        let Item::Line(label, parts) = it else { return Some(0) };
        let w: u16 = parts.iter().map(|(s, _)| width(s)).sum();
        [label_w, width(label) + 2].into_iter().find(|x| x + w <= inner.width)
    };
    let rows = |it: &Item| if value_x(it).is_some() { 1 } else { 2 };
    // Most important rows that fit, in display order.
    let mut keep = vec![false; items.len()];
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(items[i].1));
    let mut room = inner.height.saturating_sub(mode.height());
    for i in order {
        let h = rows(&items[i].0);
        if h <= room {
            room -= h;
            keep[i] = true;
        }
    }
    let mut y = numbers(buf, inner, inner.y, v, mode);
    for (it, _) in items.iter().zip(&keep).filter(|(_, k)| **k).map(|(it, _)| it) {
        if y >= inner.bottom() {
            break;
        }
        let Item::Line(label, parts) = it else {
            y += 1;
            continue;
        };
        put(buf, inner.x, y, label, st(DIM), label_w.min(inner.width));
        let mut x = match value_x(it) {
            Some(x) => inner.x + x,
            None => {
                y += 1;
                inner.x + 2
            }
        };
        for (s, style) in parts {
            let room = inner.right().saturating_sub(x);
            x = put(buf, x, y, &truncate(s, usize::from(room)), *style, room);
        }
        y += 1;
    }
}
