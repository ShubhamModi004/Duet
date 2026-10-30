//! Duet signaling server.
//!
//! Peers connect to `GET /ws`, send a `join`, and then exchange opaque WebRTC
//! payloads with `signal` messages. Media and control traffic never touch this
//! server: once ICE completes, peers talk directly (or via TURN).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use duet_protocol::{
    is_valid_room_code, new_room_code, ClientMsg, ErrorCode, PeerId, PeerInfo, Role, ServerMsg,
    MAX_ROOM_PEERS,
};
use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, Mutex};

type Tx = mpsc::UnboundedSender<ServerMsg>;

struct Peer {
    info: PeerInfo,
    tx: Tx,
}

#[derive(Default)]
struct Rooms {
    rooms: HashMap<String, HashMap<PeerId, Peer>>,
}

#[derive(Clone, Default)]
pub struct AppState {
    rooms: Arc<Mutex<Rooms>>,
    next_id: Arc<AtomicU64>,
}

pub fn app() -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/rooms/new", get(|| async { Json(serde_json::json!({ "room": new_room_code() })) }))
        .route("/ws", get(ws_handler))
        .with_state(AppState::default())
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();

    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let text = serde_json::to_string(&msg).expect("ServerMsg serializes");
            if sink.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });

    let me = format!("p{}", state.next_id.fetch_add(1, Ordering::Relaxed) + 1);
    let mut room: Option<String> = None;

    while let Some(Ok(frame)) = stream.next().await {
        let text = match frame {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let msg = match serde_json::from_str::<ClientMsg>(&text) {
            Ok(m) => m,
            Err(e) => {
                let _ = tx.send(err(ErrorCode::BadMessage, e.to_string()));
                continue;
            }
        };
        match msg {
            ClientMsg::Ping => {
                let _ = tx.send(ServerMsg::Pong);
            }
            ClientMsg::Join { room: code, name, role } => {
                if room.is_some() {
                    let _ = tx.send(err(ErrorCode::BadMessage, "already joined".into()));
                    continue;
                }
                match join(&state, &code, &me, name, role, tx.clone()).await {
                    Ok(()) => room = Some(code),
                    Err(e) => {
                        let _ = tx.send(e);
                    }
                }
            }
            ClientMsg::Signal { to, payload } => {
                let Some(code) = &room else {
                    let _ = tx.send(err(ErrorCode::NotJoined, "join a room first".into()));
                    continue;
                };
                let rooms = state.rooms.lock().await;
                match rooms.rooms.get(code).and_then(|r| r.get(&to)) {
                    Some(peer) => {
                        let _ = peer.tx.send(ServerMsg::Signal { from: me.clone(), payload });
                    }
                    None => {
                        let _ = tx.send(err(ErrorCode::UnknownPeer, format!("no peer {to}")));
                    }
                }
            }
            ClientMsg::Leave => break,
        }
    }

    if let Some(code) = room {
        leave(&state, &code, &me).await;
    }
    writer.abort();
}

async fn join(
    state: &AppState,
    code: &str,
    me: &PeerId,
    name: String,
    role: Role,
    tx: Tx,
) -> Result<(), ServerMsg> {
    if !is_valid_room_code(code) {
        return Err(err(ErrorCode::BadRoom, "room codes are lowercase letters, digits and '-'".into()));
    }
    let mut rooms = state.rooms.lock().await;
    let peers = rooms.rooms.entry(code.to_string()).or_default();
    if peers.len() >= MAX_ROOM_PEERS {
        return Err(err(ErrorCode::RoomFull, format!("max {MAX_ROOM_PEERS} people per room")));
    }
    if role == Role::Host && peers.values().any(|p| p.info.role == Role::Host) {
        return Err(err(ErrorCode::HostTaken, "this room already has a host".into()));
    }
    let info = PeerInfo { id: me.clone(), name: name.chars().take(64).collect(), role };
    let existing: Vec<PeerInfo> = peers.values().map(|p| p.info.clone()).collect();
    for p in peers.values() {
        let _ = p.tx.send(ServerMsg::PeerJoined { peer: info.clone() });
    }
    let _ = tx.send(ServerMsg::Joined { you: me.clone(), room: code.to_string(), peers: existing });
    peers.insert(me.clone(), Peer { info, tx });
    tracing::info!(room = code, peer = me, "joined");
    Ok(())
}

async fn leave(state: &AppState, code: &str, me: &PeerId) {
    let mut rooms = state.rooms.lock().await;
    if let Some(peers) = rooms.rooms.get_mut(code) {
        peers.remove(me);
        for p in peers.values() {
            let _ = p.tx.send(ServerMsg::PeerLeft { id: me.clone() });
        }
        if peers.is_empty() {
            rooms.rooms.remove(code);
        }
    }
    tracing::info!(room = code, peer = me, "left");
}

fn err(code: ErrorCode, message: String) -> ServerMsg {
    ServerMsg::Error { code, message }
}
