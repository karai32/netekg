// Colors and gradients, in true color or the nearest of 256.

use ratatui::style::Color;
use std::sync::OnceLock;

pub type Rgb = (u8, u8, u8);

pub const BG: Rgb = (10, 13, 18);
pub const BRIGHT: Rgb = (238, 242, 247);
pub const TEXT: Rgb = (196, 204, 214);
pub const DIM: Rgb = (116, 126, 140);
pub const FAINT: Rgb = (54, 62, 74);
// Empty bar track.
pub const TRACK: Rgb = (26, 31, 40);

pub const GOOD: Rgb = (74, 222, 128);
pub const WARN: Rgb = (250, 204, 21);
pub const BAD: Rgb = (248, 92, 84);
pub const HEAT: [Rgb; 3] = [GOOD, WARN, BAD];

pub const DOWN: Rgb = (56, 189, 248);
pub const UP: Rgb = (232, 121, 249);
// From the baseline to the tallest bars.
pub const DOWN_RAMP: [Rgb; 3] = [(30, 64, 175), (56, 189, 248), (200, 242, 255)];
pub const UP_RAMP: [Rgb; 3] = [(110, 30, 170), (232, 121, 249), (255, 214, 246)];

// Box colors: muted border, bright title.
pub struct Look {
    pub border: Rgb,
    pub title: Rgb,
}

pub const TRAFFIC: Look = Look { border: (36, 72, 110), title: (120, 196, 255) };
pub const CONNECTION: Look = Look { border: (34, 88, 64), title: (98, 226, 156) };
pub const NETWORK: Look = Look { border: (74, 62, 122), title: (182, 160, 255) };
pub const SPEED: Look = Look { border: (108, 78, 32), title: (255, 196, 92) };

pub fn c(rgb: Rgb) -> Color {
    static TC: OnceLock<bool> = OnceLock::new();
    if *TC.get_or_init(truecolor) { Color::Rgb(rgb.0, rgb.1, rgb.2) } else { Color::Indexed(nearest(rgb)) }
}

pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * t).round() as u8;
    (ch(a.0, b.0), ch(a.1, b.1), ch(a.2, b.2))
}

// A color along evenly spaced stops, t from 0 to 1.
pub fn ramp(stops: &[Rgb], t: f64) -> Rgb {
    match stops {
        [] => TEXT,
        [only] => *only,
        _ => {
            let pos = t.clamp(0.0, 1.0) * (stops.len() - 1) as f64;
            let i = (pos.floor() as usize).min(stops.len() - 2);
            mix(stops[i], stops[i + 1], pos - i as f64)
        }
    }
}

fn truecolor() -> bool {
    if cfg!(windows) {
        return true;
    }
    let ct = std::env::var("COLORTERM").unwrap_or_default().to_ascii_lowercase();
    if ct.contains("truecolor") || ct.contains("24bit") {
        return true;
    }
    if std::env::var("TERM_PROGRAM").is_ok_and(|v| v == "Apple_Terminal") {
        return false;
    }
    !std::env::var("TERM").is_ok_and(|t| t == "linux")
}

// Nearest xterm 256-color index for terminals without true color.
fn nearest((r, g, b): Rgb) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let dist = |a: (u8, u8, u8)| {
        let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).pow(2);
        d(a.0, r) + d(a.1, g) + d(a.2, b)
    };
    let idx = |v: u8| {
        LEVELS.iter().enumerate().min_by_key(|(_, l)| (i32::from(**l) - i32::from(v)).abs()).map_or(0, |(i, _)| i)
    };
    let (ri, gi, bi) = (idx(r), idx(g), idx(b));
    let cube = (16 + 36 * ri + 6 * gi + bi) as u8;
    let cube_d = dist((LEVELS[ri], LEVELS[gi], LEVELS[bi]));
    let gray_i = ((i32::from(r) + i32::from(g) + i32::from(b)) / 3 - 8).clamp(0, 230) / 10;
    let gv = (8 + gray_i * 10) as u8;
    if dist((gv, gv, gv)) < cube_d { 232 + gray_i as u8 } else { cube }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_colors() {
        assert_eq!(nearest((0, 0, 0)), 16);
        assert_eq!(nearest((255, 255, 255)), 231);
        assert_eq!(nearest((255, 0, 0)), 196);
        assert_eq!(nearest((128, 128, 128)), 244);
    }

    #[test]
    fn gradients() {
        assert_eq!(mix((0, 0, 0), (200, 100, 50), 0.5), (100, 50, 25));
        assert_eq!(ramp(&HEAT, 0.0), GOOD);
        assert_eq!(ramp(&HEAT, 0.5), WARN);
        assert_eq!(ramp(&HEAT, 1.0), BAD);
        assert_eq!(ramp(&HEAT, 7.0), BAD);
    }
}
