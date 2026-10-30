// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;

use duet_core::{HostConfig, HostEvent, HostHandle};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

const SIGNAL_PORT: u16 = 8787;

#[derive(Default)]
struct AppState {
    host: Mutex<Option<HostHandle>>,
    local_server: Mutex<bool>,
}

#[derive(Serialize)]
struct Hosting {
    room: String,
    /// Addresses guests can paste into "Server" to reach the built-in server.
    join_servers: Vec<String>,
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
        .map_err(|e| e.to_string())?;
    let room = handle.room.clone();
    *state.host.lock().unwrap() = Some(handle);

    show_overlay(&app);
    let events_app = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let stopped = matches!(ev, HostEvent::Stopped);
            let _ = events_app.emit("host-event", &ev);
            if stopped {
                events_app.state::<AppState>().host.lock().unwrap().take();
                hide_overlay(&events_app);
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
fn platform() -> &'static str {
    std::env::consts::OS
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

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            host_start,
            host_stop,
            host_grant,
            host_deny,
            host_revoke,
            host_set_auto_grant,
            host_set_muted,
            platform,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Duet");
}
