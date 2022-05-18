// Stops console from showing, but also stops stdout and stderr
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gui;
mod util;

use clap::Parser;

#[derive(Parser)]
struct Opt {
    #[clap(subcommand)]
    sub: Option<Subcommand>,
}

#[derive(Parser)]
enum Subcommand {
    Gui(gui::Gui),
}

fn main() {
    let opt = Opt::parse();
    match opt.sub {
        Some(Subcommand::Gui(args)) => gui::gui(args),
        None => gui::gui(Default::default()),
    }
}
