// Hide the extra console window Windows would otherwise open alongside the GUI in
// release builds. No-op on macOS/Linux; debug builds keep the console so `tauri dev`
// stderr logging stays visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    socai_app_lib::run()
}
