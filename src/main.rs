mod app;
mod cloud_init;
mod git;
mod network;
mod ports;
mod runtime;
mod state;

use std::process;

fn main() {
    if let Err(err) = app::run() {
        eprintln!("error: {err}");
        process::exit(1);
    }
}
