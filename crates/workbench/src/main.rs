//! Desktop management interface for the Suzu Resident.
//!
//! Per ADR-0002, the Resident is the single
//! writer — this shell never touches a serial port. It is also the
//! only client of the loopback API: the webview calls Tauri commands
//! and the Rust side makes the HTTP call, so no browser CORS exists
//! anywhere in the product and no unrelated webpage can control devices.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};

const MAIN_WINDOW: &str = "main";
const PORT_NUM: u16 = 7899;

/// Set once the webview is listening. The bridge reconnects so the
/// the next connection's opening snapshot reaches a listener instead of
/// an absent listener. The bridge connects before the window loads; without
/// this, the snapshot has no listener and the window remains
/// empty until the Resident next publishes a change.
static BRIDGE_RESYNC: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Resident loopback address.
fn resident() -> String {
    format!("127.0.0.1:{PORT_NUM}")
}

/// Return whether the URL is an approved product destination.
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
    // The webview's listeners are up: ask the bridge for a fresh
    // connection so the opening snapshot refreshes all client state.
    BRIDGE_RESYNC.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Open one of the About page's destinations. Anything outside the
/// approved product destinations is rejected.
#[tauri::command]
async fn open_destination(url: String) -> Result<(), String> {
    if !url_is_ours(&url) {
        return Err(format!("refused: {url} is not an approved destination"));
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
/// The webview never sends cross-origin HTTP directly.
/// preflights, and no webpage can reach the Resident through us.
/// Async on a blocking worker: a slow or absent Resident must never
/// freeze the window (the UI-thread lock, learned 2026-08-30).
#[tauri::command]
async fn api(
    method: String,
    path: String,
    body: Option<String>,
) -> Result<serde_json::Value, String> {
    if !path.starts_with("/api/") || path.contains("..") {
        return Err("refused: only /api/ paths on the Resident".into());
    }
    let payload = body.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || http_call(&method, &path, &Some(payload)))
        .await
        .map_err(|e| format!("worker: {e}"))?
        .map(|(status, body)| serde_json::json!({ "status": status, "body": body }))
}

/// Perform a raw HTTP request over the Resident loopback connection.
fn http_call(
    method: &str,
    path: &str,
    body: &Option<String>,
) -> Result<(u16, String), String> {
    use std::io::{Read, Write};
    let addr = resident();
    let mut stream = std::net::TcpStream::connect(&*addr).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    let payload = body.as_deref().unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: {addr}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
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

/// Return whether a process is accepting connections on the Resident port.
fn resident_port_is_open() -> bool {
    use std::net::SocketAddr;
    let addr: SocketAddr = resident().parse().expect("loopback addr");
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
}

/// Wait for the Resident port to accept connections.
fn wait_for_resident_port(timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if resident_port_is_open() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    false
}

/// Start the Resident as a detached process so it continues
/// running if the window closes. Probe the port first (ADR-0004); a second
/// process would fail to bind the port. The repository
/// root is derived from the binary's own location (target/debug -> the
/// project root).
#[tauri::command]
async fn start_resident() -> Result<String, String> {
    if resident_port_is_open() {
        return Err("refused: 127.0.0.1:7899 is already owned by a running process".into());
    }
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

    let pid: u32 = tauri::async_runtime::spawn_blocking(move || -> Result<u32, String> {
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
        Ok(child.id())
    }).await.map_err(|e| format!("worker: {e}"))??;

    // Startup succeeds only after the Resident accepts a connection.
    if wait_for_resident_port(std::time::Duration::from_secs(10)) {
        Ok(format!("resident started — pid {pid}"))
    } else {
        Err(format!(
            "resident process {pid} did not listen within 10 seconds; see serve.err.log"
        ))
    }
}

/// Request shutdown, then verify that the loopback port becomes free.
/// pretended.
#[tauri::command]
async fn stop_resident() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !resident_port_is_open() {
            return Ok(serde_json::json!({ "stopped": true, "was": "already down" }));
        }
        match http_call("POST", "/api/shutdown", &None) {
            Ok((200, _)) => {}
            Ok((status, body)) => {
                return Ok(serde_json::json!({
                    "stopped": false,
                    "reason": format!("the shutdown endpoint returned {status}: {body}"),
                }));
            }
            Err(e) => {
                return Ok(serde_json::json!({
                    "stopped": false,
                    "reason": format!("the shutdown endpoint failed: {e}"),
                }));
            }
        }
        // The resident exits after answering; wait briefly for the port
        // to actually free, and report what is true when it does not.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if !resident_port_is_open() {
                return Ok(serde_json::json!({ "stopped": true }));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Ok(serde_json::json!({
            "stopped": false,
            "reason": "shutdown returned successfully, but 127.0.0.1:7899 remains in use",
        }))
    })
    .await
    .map_err(|e| format!("worker: {e}"))?
}

/// Maintain one SSE connection to the Resident and republish
/// every frame as a Tauri event, so the workbench's store moves with
/// the Resident instead of polling it. The stream carries typed events
/// (`snapshot` · `fact` · `journal`); the health of the connection is
/// itself an event, because "connected" is state the store keeps
/// (ADR-0004). Reconnects are the server's business: every new
/// connection opens with a fresh snapshot, so nothing appends twice.
fn spawn_resident_events(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        let ok = (|| -> std::io::Result<()> {
            use std::io::{Read, Write};
            use std::net::SocketAddr;
            use std::time::Instant;
            let addr: SocketAddr = resident().parse().expect("loopback addr");
            let mut stream = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(2))?;
            // Short read timeouts keep the loop responsive to
            // resync requests. Fifteen seconds
            // without a byte (the Resident pings every 10 seconds) means disconnect.
            stream.set_read_timeout(Some(std::time::Duration::from_millis(500))).ok();
            stream.write_all(
                format!("GET /api/events HTTP/1.1\r\nhost: {}\r\naccept: text/event-stream\r\nconnection: close\r\n\r\n", resident())
                    .as_bytes(),
            )?;
            stream.flush()?;
            let _ = app.emit("resident-health", "connected");
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 1024];
            let mut last_rx = Instant::now();
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        last_rx = Instant::now();
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        // Windows reports read timeouts as TimedOut; Unix uses WouldBlock.
                        if last_rx.elapsed() > std::time::Duration::from_secs(15) {
                            break; // heartbeat timeout
                        }
                        if BRIDGE_RESYNC.swap(false, std::sync::atomic::Ordering::Relaxed) {
                            break; // the UI requested a fresh snapshot
                        }
                        continue;
                    }
                    Err(_) => break,
                }
                // One SSE frame = optional `event:` line + `data:` line,
                // closed by a blank line. Comments (`: ping`) are dropped;
                // The JSON payload contains its own type tag.
                while let Some(pos) = buf.windows(2).position(|w| w == b"\n\n") {
                    let frame: Vec<u8> = buf.drain(..pos + 2).collect();
                    let text = String::from_utf8_lossy(&frame);
                    let mut payload = None;
                    for line in text.lines() {
                        if let Some(rest) = line.strip_prefix("data: ") {
                            payload = Some(rest.trim_end().to_string());
                        }
                    }
                    if let Some(data) = payload {
                        let _ = app.emit("resident-event", data);
                    }
                }
            }
            Ok(())
        })();
        let _ = ok; // the Resident may be down; we keep trying
        let _ = app.emit("resident-health", "reconnecting");
        std::thread::sleep(std::time::Duration::from_secs(2));
    });
}

fn main() {
    let start_minimized = std::env::args().any(|a| a == "--minimized");
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![ready, open_destination, reveal_path, start_resident, stop_resident, api])
        .setup(move |app| {
            build_tray(app)?;
            spawn_resident_events(app.handle().clone());
            // The window remains hidden until the first render completes (`ready`), so the
            // user never sees an empty frame.
            if !start_minimized {
                if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window hides it; the tray menu controls process exit.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("suzu workbench failed to run");
}

/// System tray integration. Left click shows the main window.
fn build_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Workbench", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Suzu Workbench", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    let mut builder = TrayIconBuilder::with_id("suzu")
        .icon(tauri::include_image!("icons/suzu.png"))
        .tooltip("Suzu device manager")
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
