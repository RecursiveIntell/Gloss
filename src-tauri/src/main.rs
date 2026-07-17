// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::process::Command;

fn main() {
    if let Err(e) = gloss_lib::run_inner() {
        let message = format!("{e}");
        eprintln!("[gloss] fatal: {message}");
        show_fatal_startup_error(&message);
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn show_fatal_startup_error(message: &str) {
    let msg = message.lines().next().unwrap_or(message);
    let msg_status = Command::new("msg").args(["*", msg]).status();
    if msg_status.map(|s| s.success()).unwrap_or(false) {
        return;
    }

    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; [void][System.Windows.Forms.MessageBox]::Show('{}', 'Gloss startup error')",
        msg.replace('\'', "''")
    );
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status();
}

#[cfg(target_os = "macos")]
fn show_fatal_startup_error(message: &str) {
    use serde_json::Value;

    let message = Value::String(message.to_string());
    let title = Value::String("Gloss startup error".to_string());
    let script = format!(
        "display dialog {} with title {} buttons {{\"OK\"}} default button \"OK\"",
        message, title
    );
    let _ = Command::new("osascript").args(["-e", &script]).status();
}

#[cfg(target_os = "linux")]
fn show_fatal_startup_error(message: &str) {
    let msg = message.lines().next().unwrap_or(message);
    let _ = Command::new("notify-send")
        .args(["Gloss startup error", msg])
        .status();
}
