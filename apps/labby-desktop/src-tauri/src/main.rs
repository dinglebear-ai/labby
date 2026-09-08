#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(err) = labby_desktop_lib::run() {
        eprintln!("labby desktop: fatal error: {err}");
        std::process::exit(1);
    }
}
