// Prevent a console window from popping up on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = hpbar_lib::platform::run_ssh_askpass() {
        std::process::exit(code);
    }
    hpbar_lib::run()
}
