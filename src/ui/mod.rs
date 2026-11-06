// Screen drawing.

mod page;
pub mod theme;
pub mod widgets;

use crate::app::{App, Public, View};
use crate::verdict::Health;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use theme::DIM;
use widgets::{fill, put, st, width};

pub const MIN_W: u16 = 50;
pub const MIN_H: u16 = 14;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let view = app.view();
    app.animate = view.speed.running() || view.diag.health == Health::Measuring;
    render(f.buffer_mut(), area, app, &view);
}

pub fn render(buf: &mut Buffer, area: Rect, app: &mut App, v: &View) {
    app.hits.clear();
    fill(buf, area);
    if area.width < MIN_W || area.height < MIN_H {
        let msg = format!("Make the window bigger ({MIN_W}×{MIN_H})");
        let x = area.x + area.width.saturating_sub(width(&msg)) / 2;
        put(buf, x, area.y + area.height / 2, &msg, st(DIM), area.width);
        return;
    }
    page::draw(buf, area, app, v);
}

// Speed test server as "City (IATA)".
fn server_name(v: &View) -> Option<String> {
    let colo = v.speed.res.server.clone()?;
    let city = match &v.public {
        Public::Ready(p) if p.colo.as_deref() == Some(colo.as_str()) => p.colo_city.clone(),
        _ => None,
    };
    Some(match city {
        Some(c) => format!("{c} ({colo})"),
        None => colo,
    })
}

#[cfg(test)]
mod tests;
