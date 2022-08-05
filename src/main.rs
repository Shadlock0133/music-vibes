// Stops console from showing, but also stops stdout and stderr
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gui;
mod settings;
mod util;

use clap::Parser;
use gui::Gui;

fn main() {
    let args = Gui::parse();
    gui::gui(args);
}
