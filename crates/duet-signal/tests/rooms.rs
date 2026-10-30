use duet_protocol::{ClientMsg, ErrorCode, Role, ServerMsg};
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn spawn_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, duet_signal::app()).await.unwrap() });
    format!("ws://{addr}/ws")
}

async fn send(ws: &mut Ws, m: ClientMsg) {
    ws.send(Message::Text(serde_json::to_string(&m).unwrap())).await.unwrap();
}

async fn recv(ws: &mut Ws) -> ServerMsg {
    loop {
        match ws.next().await.unwrap().unwrap() {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            _ => continue,
        }
    }
}

async fn join(url: &str, room: &str, name: &str, role: Role) -> (Ws, ServerMsg) {
    let (mut ws, _) = connect_async(url).await.unwrap();
    send(&mut ws, ClientMsg::Join { room: room.into(), name: name.into(), role }).await;
    let first = recv(&mut ws).await;
    (ws, first)
}

#[tokio::test]
async fn host_and_guest_exchange_signals() {
    let url = spawn_server().await;

    let (mut host, joined) = join(&url, "kip-moza-tule", "Host", Role::Host).await;
    let ServerMsg::Joined { you: host_id, peers, .. } = joined else { panic!("{joined:?}") };
    assert!(peers.is_empty());

    let (mut guest, joined) = join(&url, "kip-moza-tule", "Guest", Role::Guest).await;
    let ServerMsg::Joined { you: guest_id, peers, .. } = joined else { panic!("{joined:?}") };
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].id, host_id);

    let ServerMsg::PeerJoined { peer } = recv(&mut host).await else { panic!() };
    assert_eq!(peer.id, guest_id);
    assert_eq!(peer.role, Role::Guest);

    let offer = json!({"sdp": {"type": "offer", "sdp": "v=0..."}});
    send(&mut host, ClientMsg::Signal { to: guest_id.clone(), payload: offer.clone() }).await;
    assert_eq!(recv(&mut guest).await, ServerMsg::Signal { from: host_id.clone(), payload: offer });

    send(&mut guest, ClientMsg::Leave).await;
    assert_eq!(recv(&mut host).await, ServerMsg::PeerLeft { id: guest_id });
}

#[tokio::test]
async fn second_host_is_rejected() {
    let url = spawn_server().await;
    let (_h, _) = join(&url, "room-1", "A", Role::Host).await;
    let (_h2, resp) = join(&url, "room-1", "B", Role::Host).await;
    assert!(matches!(resp, ServerMsg::Error { code: ErrorCode::HostTaken, .. }), "{resp:?}");
}

#[tokio::test]
async fn bad_room_and_bad_json_are_reported() {
    let url = spawn_server().await;
    let (_ws, resp) = join(&url, "Not A Room!", "A", Role::Guest).await;
    assert!(matches!(resp, ServerMsg::Error { code: ErrorCode::BadRoom, .. }), "{resp:?}");

    let (mut ws, _) = connect_async(&url).await.unwrap();
    ws.send(Message::Text("{nope".into())).await.unwrap();
    assert!(matches!(recv(&mut ws).await, ServerMsg::Error { code: ErrorCode::BadMessage, .. }));
}

#[tokio::test]
async fn room_is_capped() {
    let url = spawn_server().await;
    let mut keep = vec![];
    for i in 0..duet_protocol::MAX_ROOM_PEERS {
        let (ws, resp) = join(&url, "full-room", &format!("g{i}"), Role::Guest).await;
        assert!(matches!(resp, ServerMsg::Joined { .. }));
        keep.push(ws);
    }
    let (_ws, resp) = join(&url, "full-room", "late", Role::Guest).await;
    assert!(matches!(resp, ServerMsg::Error { code: ErrorCode::RoomFull, .. }), "{resp:?}");
}
