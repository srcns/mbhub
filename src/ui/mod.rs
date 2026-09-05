//! Rendering entry point. Splits the frame into the green header bar, the
//! active screen's body (or full-screen markdown viewer), and the green footer
//! reference bar.

pub mod footer;
pub mod header;
pub mod markdown;
pub mod memory;
pub mod search;
pub mod settings;
pub mod viewer;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::Frame;

use crate::app::{App, Screen};

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let rows = Layout::vertical([
        Constraint::Length(1), // green header bar
        Constraint::Min(0),    // body
        Constraint::Length(1), // green footer bar
    ])
    .split(area);

    header::draw(frame, rows[0], app);

    let body: Rect = rows[1];
    app.body_width.set(body.width as usize);
    app.body_height.set(body.height as usize);

    if let Some(viewer) = &app.viewer {
        viewer::draw(frame, body, viewer);
    } else {
        match app.screen {
            Screen::Search => search::draw(frame, body, app),
            Screen::Memory => memory::draw(frame, body, app),
            Screen::Settings => settings::draw(frame, body, app),
        }
    }

    footer::draw(frame, rows[2], app);
}
