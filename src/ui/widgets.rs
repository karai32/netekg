// Drawing helpers: text, boxes, big digits, gradient charts and bars.

use super::theme::{BG, Look, Rgb, TRACK, c, mix, ramp};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

pub type Piece = (String, Style);

pub fn st(fg: Rgb) -> Style {
    Style::new().fg(c(fg)).bg(c(BG))
}

pub fn bold(fg: Rgb) -> Style {
    st(fg).add_modifier(Modifier::BOLD)
}

// Dark text on a colored background, for badges.
pub fn badge(bg: Rgb) -> Style {
    Style::new().fg(c(BG)).bg(c(bg)).add_modifier(Modifier::BOLD)
}

pub fn fill(buf: &mut Buffer, area: Rect) {
    let area = area.intersection(buf.area);
    let bg = c(BG);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].reset();
            buf[(x, y)].set_bg(bg);
        }
    }
}

// Text clipped to `max` cells; returns the x after it.
pub fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, style: Style, max: u16) -> u16 {
    let a = buf.area;
    if y < a.top() || y >= a.bottom() || x < a.left() || x >= a.right() || max == 0 {
        return x;
    }
    buf.set_stringn(x, y, s, usize::from(max), style).0
}

// Writes styled pieces on one line, clipped to `right`.
pub fn spans<S: AsRef<str>>(buf: &mut Buffer, mut x: u16, y: u16, parts: &[(S, Style)], right: u16) -> u16 {
    for (s, style) in parts {
        if x >= right {
            break;
        }
        x = put(buf, x, y, s.as_ref(), *style, right - x);
    }
    x
}

pub fn set(buf: &mut Buffer, x: u16, y: u16, sym: &str, style: Style) {
    if buf.area.contains((x, y).into()) {
        buf[(x, y)].set_symbol(sym).set_style(style);
    }
}

pub fn width(s: &str) -> u16 {
    u16::try_from(s.chars().count()).unwrap_or(u16::MAX)
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

pub fn wrap(text: &str, w: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let need = cur.chars().count() + usize::from(!cur.is_empty()) + word.chars().count();
        if !cur.is_empty() && need > w {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

// What a box shows in its frame besides the title.
#[derive(Default)]
pub struct Border {
    // Top edge, on the right.
    pub status: Vec<Piece>,
    // Bottom edge: a note on the left, key hints on the right.
    pub note: Vec<Piece>,
    pub keys: Vec<Piece>,
}

// Rounded box; returns the padded inner area.
pub fn boxed(buf: &mut Buffer, area: Rect, look: &Look, title: &str, border: &Border) -> Rect {
    let area = area.intersection(buf.area);
    if area.width < 6 || area.height < 2 {
        return Rect::default();
    }
    let line = st(look.border);
    let (l, r, t, b) = (area.left(), area.right() - 1, area.top(), area.bottom() - 1);
    for x in l..=r {
        set(buf, x, t, "─", line);
        set(buf, x, b, "─", line);
    }
    for y in t..=b {
        set(buf, l, y, "│", line);
        set(buf, r, y, "│", line);
    }
    set(buf, l, t, "╭", line);
    set(buf, r, t, "╮", line);
    set(buf, l, b, "╰", line);
    set(buf, r, b, "╯", line);
    let title_end = put(buf, l + 2, t, &format!(" {title} "), bold(look.title), r.saturating_sub(l + 3));
    // Right-aligned in the border, skipped when they don't fit.
    let edge = |buf: &mut Buffer, y: u16, parts: &[Piece], after: u16| -> u16 {
        let w = parts.iter().map(|(s, _)| width(s)).sum::<u16>();
        if parts.is_empty() || r < after + w + 4 {
            return r;
        }
        let start = r - 3 - w;
        put(buf, start, y, " ", line, 1);
        let x = spans(buf, start + 1, y, parts, r - 2);
        put(buf, x, y, " ", line, 1);
        start
    };
    edge(buf, t, &border.status, title_end);
    let keys_at = edge(buf, b, &border.keys, l + 1);
    if !border.note.is_empty() && keys_at > l + 6 {
        let x = put(buf, l + 2, b, " ", line, 1);
        let x = spans(buf, x, b, &border.note, keys_at - 2);
        put(buf, x, b, " ", line, 1);
    }
    Rect::new(l + 2, t + 1, area.width - 4, area.height - 2)
}

// 3x5 pixel font, one byte per row, leftmost pixel in the highest bit.
fn glyph(c: char) -> Option<(u8, [u8; 5])> {
    Some(match c {
        '0' => (3, [7, 5, 5, 5, 7]),
        '1' => (3, [2, 6, 2, 2, 7]),
        '2' => (3, [7, 1, 7, 4, 7]),
        '3' => (3, [7, 1, 7, 1, 7]),
        '4' => (3, [5, 5, 7, 1, 1]),
        '5' => (3, [7, 4, 7, 1, 7]),
        '6' => (3, [7, 4, 7, 5, 7]),
        '7' => (3, [7, 1, 1, 2, 2]),
        '8' => (3, [7, 5, 7, 5, 7]),
        '9' => (3, [7, 5, 7, 1, 7]),
        '.' => (1, [0, 0, 0, 0, 1]),
        '-' => (3, [0, 0, 7, 0, 0]),
        ' ' => (1, [0, 0, 0, 0, 0]),
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Size {
    // Half blocks: one cell per pixel column, two pixel rows per cell; 3 rows tall.
    Small,
    // Full blocks: two cells per pixel, one row per pixel; 5 rows tall.
    Large,
}

impl Size {
    pub fn height(self) -> u16 {
        match self {
            Size::Small => 3,
            Size::Large => 5,
        }
    }
}

pub fn big_width(s: &str, size: Size) -> u16 {
    let px: u16 = s.chars().filter_map(glyph).map(|(w, _)| u16::from(w) + 1).sum::<u16>().saturating_sub(1);
    match size {
        Size::Small => px,
        Size::Large => px * 2,
    }
}

// Big digits shaded from `top` to `bottom`.
pub fn big(buf: &mut Buffer, x: u16, y: u16, s: &str, (top, bottom): (Rgb, Rgb), size: Size) {
    let rows = size.height();
    let style = |row: u16| bold(mix(top, bottom, f64::from(row) / f64::from(rows - 1)));
    let mut cx = x;
    for (w, pixels) in s.chars().filter_map(glyph) {
        for col in 0..w {
            let bit = |r: usize| pixels[r] >> (w - 1 - col) & 1 == 1;
            match size {
                Size::Small => {
                    for cell in 0..3 {
                        let lower = cell * 2 + 1 < 5 && bit(cell * 2 + 1);
                        let sym = match (bit(cell * 2), lower) {
                            (true, true) => "█",
                            (true, false) => "▀",
                            (false, true) => "▄",
                            (false, false) => continue,
                        };
                        set(buf, cx + u16::from(col), y + cell as u16, sym, style(cell as u16));
                    }
                }
                Size::Large => {
                    for r in 0..5 {
                        if bit(r) {
                            let px = cx + u16::from(col) * 2;
                            set(buf, px, y + r as u16, "█", style(r as u16));
                            set(buf, px + 1, y + r as u16, "█", style(r as u16));
                        }
                    }
                }
            }
        }
        cx += match size {
            Size::Small => u16::from(w) + 1,
            Size::Large => (u16::from(w) + 1) * 2,
        };
    }
}

const LOWER: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

// Eighths of a cell for a 0..1 value; at least one above zero.
fn eighths(v: f64, cells: u16) -> i32 {
    let e = (v.clamp(0.0, 1.0) * f64::from(cells) * 8.0).round() as i32;
    if v > 0.0 && e == 0 { 1 } else { e }
}

// Bars growing up, colored by height.
pub fn bars_up(buf: &mut Buffer, area: Rect, values: &[f64], stops: &[Rgb]) {
    let h = area.height;
    for (i, v) in values.iter().enumerate().take(usize::from(area.width)) {
        let e = eighths(*v, h);
        for row in 0..h {
            let lvl = (e - i32::from(row) * 8).clamp(0, 8) as usize;
            if lvl == 0 {
                break;
            }
            let color = ramp(stops, f64::from(row + 1) / f64::from(h));
            set(buf, area.x + i as u16, area.bottom() - 1 - row, LOWER[lvl], st(color));
        }
    }
}

// Bars hanging down; partial cells are drawn inverted.
pub fn bars_down(buf: &mut Buffer, area: Rect, values: &[f64], stops: &[Rgb]) {
    let h = area.height;
    for (i, v) in values.iter().enumerate().take(usize::from(area.width)) {
        let e = eighths(*v, h);
        for row in 0..h {
            let lvl = (e - i32::from(row) * 8).clamp(0, 8) as usize;
            if lvl == 0 {
                break;
            }
            let color = ramp(stops, f64::from(row + 1) / f64::from(h));
            let (sym, style) =
                if lvl == 8 { ("█", st(color)) } else { (LOWER[8 - lvl], Style::new().fg(c(BG)).bg(c(color))) };
            set(buf, area.x + i as u16, area.y + row, sym, style);
        }
    }
}

// Slim progress bar; `color` gets the position from 0 to 1.
pub fn bar(buf: &mut Buffer, x: u16, y: u16, w: u16, frac: f64, color: impl Fn(f64) -> Rgb) {
    let halves = (frac.clamp(0.0, 1.0) * f64::from(w) * 2.0).round() as u32;
    for i in 0..w {
        let filled = halves.saturating_sub(u32::from(i) * 2).min(2);
        let fg = color((f64::from(i) + 0.5) / f64::from(w));
        match filled {
            2 => set(buf, x + i, y, "━", st(fg)),
            1 => set(buf, x + i, y, "╸", st(fg)),
            _ => set(buf, x + i, y, "━", st(TRACK)),
        }
    }
}

// One-row sparkline cell for a value in 0..1.
pub fn spark(v: f64) -> &'static str {
    LOWER[((v.clamp(0.0, 1.0) * 7.0).round() as usize + 1).min(8)]
}

pub fn spinner(ms: u128) -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    FRAMES[(ms / 100 % 10) as usize]
}

// Rounds a chart ceiling up to 1, 2 or 5 times a power of ten.
pub fn nice_ceil(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let p = 10f64.powf(v.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if v <= m * p {
            return m * p;
        }
    }
    10.0 * p
}

#[cfg(test)]
mod tests {
    use super::super::theme::{CONNECTION, DOWN_RAMP, TEXT};
    use super::*;

    fn text(buf: &Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn big_digit_widths() {
        assert_eq!(big_width("501", Size::Small), 11);
        assert_eq!(big_width("501", Size::Large), 22);
        assert_eq!(big_width("9.5", Size::Small), 9);
        assert_eq!(big_width("88.8", Size::Small), 13);
    }

    #[test]
    fn wrapping_and_truncation() {
        assert_eq!(wrap("one two three four", 9), vec!["one two", "three", "four"]);
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }

    #[test]
    fn nice_numbers() {
        assert_eq!(nice_ceil(0.0), 1.0);
        assert_eq!(nice_ceil(3.2), 5.0);
        assert_eq!(nice_ceil(45.0), 50.0);
        assert_eq!(nice_ceil(120.0), 200.0);
    }

    #[test]
    fn box_frame_carries_title_status_and_keys() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 4));
        let border = Border {
            status: vec![("live".into(), st(TEXT))],
            note: vec![("note".into(), st(TEXT))],
            keys: vec![("q quit".into(), st(TEXT))],
        };
        let area = buf.area;
        let inner = boxed(&mut buf, area, &CONNECTION, "title", &border);
        assert_eq!(inner, Rect::new(2, 1, 36, 2));
        let s = text(&buf);
        let (top, bottom) = (&s[..s.char_indices().nth(40).unwrap().0], s.chars().skip(120).collect::<String>());
        assert!(top.starts_with("╭─ title ─") && top.ends_with("─ live ─╮"), "{top}");
        assert!(bottom.starts_with("╰─ note ─") && bottom.ends_with("─ q quit ─╯"), "{bottom}");
    }

    #[test]
    fn helpers_never_write_outside() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        big(&mut buf, 7, 1, "888", (TEXT, TEXT), Size::Large);
        bars_up(&mut buf, Rect::new(0, 0, 3, 3), &[1.0, 0.5], &DOWN_RAMP);
        bars_down(&mut buf, Rect::new(0, 0, 3, 3), &[0.3], &DOWN_RAMP);
        bar(&mut buf, 6, 0, 9, 0.5, |f| ramp(&DOWN_RAMP, f));
        put(&mut buf, 0, 5, "x", st(TEXT), 100);
        put(&mut buf, 8, 1, "hello", st(TEXT), 100);
        assert_eq!(buf[(8, 1)].symbol(), "h");
        assert_eq!(buf[(9, 1)].symbol(), "e");
        assert_eq!(buf[(0, 2)].symbol(), "█");
    }
}
