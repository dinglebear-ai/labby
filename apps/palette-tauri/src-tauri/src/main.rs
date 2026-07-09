#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(err) = labby_palette_tauri_lib::run() {
        eprintln!("labby palette: fatal error: {err}");
        std::process::exit(1);
    }
}
