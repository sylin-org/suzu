//! Suzu's desktop workbench: the keeper's window onto the Resident.
//!
//! The house rules it follows (ADR-0002): the Resident is the single
//! writer — this shell never touches a serial port. It is also the
//! only speaker to the loopback API: the webview calls Tauri commands
//! and the Rust side makes the HTTP call, so no browser CORS exists
//! anywhere in the product and no stray webpage can drive devices.
//! Visual language and desktop patterns are ported from the family
//! (Ghostlight's workbench, koi-desktop's tray) and re-skinned with
//! suzu's own gold.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};

const MAIN_WINDOW: &str = "main";
const RESIDENT: &str = "127.0.0.1:7899";

/// Where the workbench may send a click. The closed vocabulary: a URL
/// is only ever opened if it points at the product's own surfaces —
/// the same law Ghostlight's Rust asserts over its own markup.
fn url_is_ours(url: &str) -> bool {
    url.starts_with("https://sylin.org/")
        || url.starts_with("https://github.com/sylin-org/")
}

#[tauri::command]
fn ready(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Open one of the About page's destinations. Anything outside the
/// product's own surfaces is refused, not merely unlinked.
#[tauri::command]
async fn open_destination(url: String) -> Result<(), String> {
    if !url_is_ours(&url) {
        return Err(format!("refused: {url} points outside the product's own surfaces"));
    }
    tauri::async_runtime::spawn_blocking(move || open_externally(&url))
        .await
        .map_err(|e| format!("worker: {e}"))?
}

/// Reveal a captures folder in the system file manager.
#[tauri::command]
async fn reveal_path(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || open_externally(&path))
        .await
        .map_err(|e| format!("worker: {e}"))?
}

fn open_externally(target: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", "", target])
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// One HTTP round trip to the Resident, performed by the Rust side.
/// The webview never speaks cross-origin HTTP — no CORS, no
/// preflights, and no webpage can reach the Resident through us.
/// Async on a blocking worker: a slow or absent Resident must never
/// freeze the window (the UI-thread lock, learned 2026-08-30).
#[tauri::command]
async fn api(
    method: String,
    path: String,
    body: Option<String>,
) -> Result<serde_json::Value, String> {
    use std::io::{Read, Write};
    if !path.starts_with("/api/") || path.contains("..") {
        return Err("refused: only /api/ paths on the Resident".into());
    }
    let payload = body.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || http_call(&method, &path, &Some(payload)))
        .await
        .map_err(|e| format!("worker: {e}"))?
        .map(|(status, body)| serde_json::json!({ "status": status, "body": body }))
}

/// The raw loopback round trip every command bottoms out in.
fn http_call(
    method: &str,
    path: &str,
    body: &Option<String>,
) -> Result<(u16, String), String> {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(RESIDENT).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    let payload = body.as_deref().unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: {RESIDENT}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf).to_string();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Ok((status, body))
}

/// Bring the Resident up, beside this workbench, detached: it keeps
/// running if the window closes. The repo root is derived from the
/// binary's own location (target/debug -> the project root).
#[tauri::command]
async fn start_resident() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("no exe directory")?.to_path_buf();
    let suzu = dir.join("suzu.exe");
    if !suzu.exists() {
        return Err(format!("{} not found - install the CLI beside the workbench", suzu.display()));
    }
    let repo = dir
        .ancestors()
        .nth(2)
        .map(|p| p.to_path_buf())
        .filter(|p| p.join("hardware/classes").exists())
        .ok_or("the project root was not found above the workbench binary")?;

    tauri::async_runtime::spawn_blocking(move || {
        use std::process::{Command, Stdio};
        let log = std::fs::OpenOptions::new().create(true).append(true)
            .open(repo.join("serve.log")).map_err(|e| e.to_string())?;
        let log_err = std::fs::OpenOptions::new().create(true).append(true)
            .open(repo.join("serve.err.log")).map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&suzu);
        cmd.arg("serve").current_dir(&repo)
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn().map_err(|e| e.to_string())?;
        Ok(format!("pid {}", child.id()))
    }).await.map_err(|e| format!("worker: {e}"))?
}

/// Ask the Resident to rest: a graceful shutdown, ports released, the
/// faces fall to their gardens.
#[tauri::command]
async fn stop_resident() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        http_call("POST", "/api/shutdown", &None)
    }).await.map_err(|e| format!("worker: {e}"))?
    .and_then(|(status, body)| {
        if status == 200 { Ok(serde_json::json!({ "stopping": true })) }
        else { Err(body) }
    })
}

/// The live wire: hold one SSE connection to the Resident and republish
/// every fact as a Tauri event, so the workbench's roster moves with
/// the house instead of polling it.
fn spawn_house_events(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        let ok = (|| -> std::io::Result<()> {
            use std::io::{Read, Write};
            let mut stream = std::net::TcpStream::connect(RESIDENT)?;
            stream.set_read_timeout(Some(std::time::Duration::from_secs(3600)))
                .ok();
            stream.write_all(
                format!("GET /api/events HTTP/1.1\r\nhost: {RESIDENT}\r\nconnection: close\r\n\r\n")
                    .as_bytes(),
            )?;
            stream.flush()?;
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(_) => break,
                }
                while let Some(pos) = buf.windows(2).position(|w| w == b"\n\n") {
                    let frame: Vec<u8> = buf.drain(..pos + 2).collect();
                    let text = String::from_utf8_lossy(&frame);
                    if let Some(payload) = text
                        .strip_prefix("data: ")
                        .map(str::trim_end)
                        .map(str::to_string)
                    {
                        let _ = app.emit("house", payload);
                    }
                }
            }
            Ok(())
        })();
        let _ = ok; // the Resident may be down; we keep trying
        std::thread::sleep(std::time::Duration::from_secs(3));
    });
}

fn main() {
    let start_minimized = std::env::args().any(|a| a == "--minimized");
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![ready, open_destination, reveal_path, start_resident, stop_resident, api])
        .setup(move |app| {
            build_tray(app)?;
            spawn_house_events(app.handle().clone());
            // The window is configured but hidden: the surface shows
            // itself once the first render is up (`ready`), so the
            // keeper never sees an empty frame.
            if !start_minimized {
                if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the Resident company in the
            // tray; Quit is a tray decision.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("suzu workbench failed to run");
}

/// The tray: the bell stays in the system's ear even with the window
/// shut. Left click reveals; the menu says who is running.
fn build_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Workbench", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Suzu Workbench", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    let mut builder = TrayIconBuilder::with_id("suzu")
        .icon(tauri::include_image!("icons/suzu.png"))
        .tooltip("Suzu — the garden keeps breathing")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "quit" => std::process::exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
