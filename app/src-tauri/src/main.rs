// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Duet lives in the menu bar (macOS) or system tray (Windows). Clicking the
//! icon drops down a small panel for sharing and joining; a guest's view of
//! the shared screen opens in its own window.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use duet_core::{HostConfig, HostEvent, HostHandle};
use serde::Serialize;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_positioner::{Position, WindowExt};

const SIGNAL_PORT: u16 = 8787;
const TRAY_ID: &str = "duet";
const PANEL: &str = "main";
const VIEWER: &str = "viewer";

#[derive(Default)]
struct AppState {
    host: Mutex<Option<HostHandle>>,
    local_server: Mutex<bool>,
    /// While set, the panel stays open when it loses focus (e.g. while macOS
    /// shows a permission prompt during "Start sharing").
    panel_pinned: AtomicBool,
    /// When the panel last hid itself on blur, so the tray click that caused
    /// the blur doesn't immediately reopen it.
    panel_hidden_at_ms: AtomicU64,
}

#[derive(Serialize)]
struct Hosting {
    room: String,
    /// Addresses guests can paste into "Server" to reach the built-in server.
    join_servers: Vec<String>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Starts the built-in signaling server on this machine (once).
async fn ensure_local_server(state: &AppState) -> Result<(), String> {
    {
        let mut running = state.local_server.lock().unwrap();
        if *running {
            return Ok(());
        }
        *running = true;
    }
    let listener = match tokio::net::TcpListener::bind(("0.0.0.0", SIGNAL_PORT)).await {
        Ok(l) => l,
        Err(e) => {
            *state.local_server.lock().unwrap() = false;
            return Err(format!("couldn't start the built-in server on port {SIGNAL_PORT}: {e}"));
        }
    };
    tauri::async_runtime::spawn(async move {
        if let Err(e) = axum::serve(listener, duet_signal::app()).await {
            tracing::error!("signaling server stopped: {e}");
        }
    });
    Ok(())
}

fn lan_servers() -> Vec<String> {
    let mut out: Vec<String> = local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, ip)| match ip {
            std::net::IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_link_local() => {
                Some(format!("ws://{v4}:{SIGNAL_PORT}/ws"))
            }
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

#[tauri::command]
async fn host_start(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    server: Option<String>,
) -> Result<Hosting, String> {
    if state.host.lock().unwrap().is_some() {
        return Err("already hosting".into());
    }
    let (server_url, join_servers) = match server.filter(|s| !s.trim().is_empty()) {
        Some(s) => (s.trim().to_string(), vec![s.trim().to_string()]),
        None => {
            ensure_local_server(&state).await?;
            (format!("ws://127.0.0.1:{SIGNAL_PORT}/ws"), lan_servers())
        }
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<HostEvent>();
    let handle = duet_core::host::start(HostConfig::new(server_url, name), tx)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let room = handle.room.clone();
    *state.host.lock().unwrap() = Some(handle);

    set_tray_live(&app, true);
    show_overlay(&app);
    let events_app = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let stopped = matches!(ev, HostEvent::Stopped);
            // Someone asking to drive needs an answer, so bring the panel up.
            if matches!(ev, HostEvent::ControlRequested { .. }) {
                show_panel(&events_app, true);
            }
            let _ = events_app.emit("host-event", &ev);
            if stopped {
                events_app.state::<AppState>().host.lock().unwrap().take();
                hide_overlay(&events_app);
                set_tray_live(&events_app, false);
                break;
            }
        }
    });

    Ok(Hosting { room, join_servers })
}

fn with_host(state: &State<'_, AppState>, f: impl FnOnce(&HostHandle)) -> Result<(), String> {
    match state.host.lock().unwrap().as_ref() {
        Some(h) => {
            f(h);
            Ok(())
        }
        None => Err("not hosting".into()),
    }
}

#[tauri::command]
fn host_stop(state: State<'_, AppState>) -> Result<(), String> {
    with_host(&state, |h| h.stop())
}

#[tauri::command]
fn host_grant(state: State<'_, AppState>, peer: String) -> Result<(), String> {
    with_host(&state, |h| h.grant(peer))
}

#[tauri::command]
fn host_deny(state: State<'_, AppState>, peer: String) -> Result<(), String> {
    with_host(&state, |h| h.deny(peer))
}

#[tauri::command]
fn host_revoke(state: State<'_, AppState>) -> Result<(), String> {
    with_host(&state, |h| h.revoke())
}

#[tauri::command]
fn host_set_auto_grant(state: State<'_, AppState>, on: bool) -> Result<(), String> {
    with_host(&state, |h| h.set_auto_grant(on))
}

#[tauri::command]
fn host_set_muted(state: State<'_, AppState>, muted: bool) -> Result<(), String> {
    with_host(&state, |h| h.set_muted(muted))
}

#[tauri::command]
fn host_clear_drawings(state: State<'_, AppState>) -> Result<(), String> {
    with_host(&state, |h| h.clear_drawings())
}

#[tauri::command]
fn platform() -> &'static str {
    std::env::consts::OS
}

#[tauri::command]
fn set_panel_pinned(state: State<'_, AppState>, pinned: bool) {
    state.panel_pinned.store(pinned, Ordering::Relaxed);
}

#[tauri::command]
fn quit(app: AppHandle) {
    if let Some(h) = app.state::<AppState>().host.lock().unwrap().as_ref() {
        h.stop();
    }
    app.exit(0);
}

/// Opens (or focuses) the window that shows the host's screen.
#[tauri::command]
fn open_viewer(app: AppHandle, room: String, server: String, name: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(VIEWER) {
        let _ = w.set_focus();
        return Ok(());
    }
    let query = format!(
        "viewer.html?room={}&server={}&name={}",
        urlencode(&room),
        urlencode(&server),
        urlencode(&name)
    );
    let w = WebviewWindowBuilder::new(&app, VIEWER, WebviewUrl::App(query.into()))
        .title(format!("Duet · {room}"))
        .inner_size(1280.0, 800.0)
        .min_inner_size(640.0, 400.0)
        .center()
        .build()
        .map_err(|e| e.to_string())?;
    // Show in the Dock and app switcher while a viewer is open.
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    let app2 = app.clone();
    w.on_window_event(move |e| {
        if let WindowEvent::Destroyed = e {
            #[cfg(target_os = "macos")]
            let _ = app2.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let _ = &app2;
        }
    });
    let _ = w.set_focus();
    hide_panel(&app);
    Ok(())
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn toggle_panel(app: &AppHandle) {
    let Some(w) = app.get_webview_window(PANEL) else { return };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
        return;
    }
    // The click that blurred (and hid) the panel shouldn't reopen it.
    let hidden_at = app.state::<AppState>().panel_hidden_at_ms.load(Ordering::Relaxed);
    if now_ms().saturating_sub(hidden_at) < 250 {
        return;
    }
    show_panel(app, true);
}

/// Shows the panel under the tray icon, or centred when it wasn't opened from
/// the tray (the tray's position is only known after it has been clicked).
fn show_panel(app: &AppHandle, under_tray: bool) {
    let Some(w) = app.get_webview_window(PANEL) else { return };
    let placed = under_tray && {
        #[cfg(target_os = "macos")]
        let pos = Position::TrayCenter;
        #[cfg(not(target_os = "macos"))]
        let pos = Position::TrayBottomCenter;
        w.move_window_constrained(pos).is_ok()
    };
    if !placed {
        let _ = w.center();
    }
    let _ = w.show();
    let _ = w.set_focus();
}

fn hide_panel(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(PANEL) {
        let _ = w.hide();
    }
}

fn set_tray_live(app: &AppHandle, live: bool) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        // macOS shows the title next to the icon; Windows ignores it.
        let _ = tray.set_title(live.then_some("LIVE"));
        let _ = tray.set_tooltip(Some(if live { "Duet · sharing your screen" } else { "Duet" }));
    }
}

/// A transparent, click-through, always-on-top window covering the primary
/// display. Guests' pointers are drawn here, so they also show up in the
/// shared screen for everyone.
fn show_overlay(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.show();
        return;
    }
    let Ok(Some(monitor)) = app.primary_monitor() else { return };
    let scale = monitor.scale_factor();
    let size = monitor.size().to_logical::<f64>(scale);
    let pos = monitor.position().to_logical::<f64>(scale);
    let built = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
        .title("Duet pointers")
        .transparent(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .shadow(false)
        .position(pos.x, pos.y)
        .inner_size(size.width, size.height)
        .build();
    match built {
        Ok(w) => {
            let _ = w.set_ignore_cursor_events(true);
        }
        Err(e) => tracing::warn!("pointer overlay unavailable: {e}"),
    }
}

fn hide_overlay(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.hide();
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Duet", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Duet", true, Some("CmdOrCtrl+Q"))?;
    let menu = Menu::with_items(app, &[&open, &quit_item])?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(include_bytes!("../icons/tray@2x.png"))?)
        .icon_as_template(true)
        .tooltip("Duet")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_panel(app, false),
            "quit" => quit(app.clone()),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } =
                event
            {
                toggle_panel(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_positioner::init())
        .manage(AppState::default())
        .setup(|app| {
            // Menu-bar app: no Dock icon until a viewer window opens.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            build_tray(app.handle())?;

            if let Some(panel) = app.get_webview_window(PANEL) {
                let handle = app.handle().clone();
                panel.on_window_event(move |e| match e {
                    WindowEvent::Focused(false) => {
                        let state = handle.state::<AppState>();
                        if !state.panel_pinned.load(Ordering::Relaxed) {
                            state.panel_hidden_at_ms.store(now_ms(), Ordering::Relaxed);
                            hide_panel(&handle);
                        }
                    }
                    WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        hide_panel(&handle);
                    }
                    _ => {}
                });
            }
            // First launch: show the panel so people find the app.
            show_panel(app.handle(), false);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            host_start,
            host_stop,
            host_grant,
            host_deny,
            host_revoke,
            host_set_auto_grant,
            host_set_muted,
            host_clear_drawings,
            platform,
            set_panel_pinned,
            open_viewer,
            quit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Duet");
}
