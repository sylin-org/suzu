//! Suzu's desktop workbench: the keeper's window onto the Resident.
//!
//! The house rules it follows (ADR-0002): the Resident is the single
//! writer — this shell never touches a serial port, it renders what
//! the loopback API answers and opens what the roster's record names.
//! Visual language and desktop patterns are ported from the family
//! (Ghostlight's workbench, koi-desktop's tray) and re-skinned with
//! suzu's own gold.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};

const MAIN_WINDOW: &str = "main";

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
fn open_destination(url: String) -> Result<(), String> {
    if !url_is_ours(&url) {
        return Err(format!("refused: {url} points outside the product's own surfaces"));
    }
    open_externally(&url)
}

/// Reveal a captures folder in the system file manager.
#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    open_externally(&path)
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

fn main() {
    let start_minimized = std::env::args().any(|a| a == "--minimized");
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![ready, open_destination, reveal_path])
        .setup(move |app| {
            build_tray(app)?;
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
