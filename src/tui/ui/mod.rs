pub mod main_menu;
pub mod scaffold;
pub mod browse;
pub mod run;

use ratatui::Frame;
use super::app::{App, Screen};

/// Top-level render dispatcher.
pub fn render(frame: &mut Frame, app: &mut App) {
    match &app.screen {
        Screen::MainMenu => main_menu::render(frame, app),
        Screen::ScaffoldChooseType => scaffold::render_choose_type(frame, app),
        Screen::ScaffoldConnection => scaffold::render_connection(frame, app),
        Screen::ScaffoldForwards => scaffold::render_forwards(frame, app),
        Screen::ScaffoldPreview => scaffold::render_preview(frame, app),
        Screen::Browse => browse::render_list(frame, app),
        Screen::BrowseDetail => browse::render_detail(frame, app),
        Screen::Run => run::render(frame, app),
    }
}
