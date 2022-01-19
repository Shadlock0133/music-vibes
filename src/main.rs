// Stops console from showing, but also stops stdout and stderr
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gui;
mod tui;
mod util;

use clap::Parser;

#[derive(Parser)]
struct Opt {
    #[clap(subcommand)]
    sub: Option<Subcommand>,
}

#[derive(Parser)]
enum Subcommand {
    Tui(tui::Tui),
    Gui(gui::Gui),
}

fn main() {
    let opt = Opt::parse();
    match opt.sub {
        Some(Subcommand::Tui(args)) => tui::tui(args),
        Some(Subcommand::Gui(args)) => gui::gui(args),
        None => gui::gui(Default::default()),
    }
}
