# Duet protocol

All messages are JSON objects tagged with `type`. The Rust definitions in
`crates/duet-protocol/src/lib.rs` are canonical.

## Signaling (WebSocket `GET /ws`)

Client → server:

```json
{"type": "join", "room": "kip-moza-tule", "name": "Ada", "role": "host"}
{"type": "signal", "to": "p2", "payload": {"sdp": {"type": "offer", "sdp": "v=0..."}}}
{"type": "signal", "to": "p1", "payload": {"ice": {"candidate": "candidate:...", "sdpMid": "0", "sdpMLineIndex": 0}}}
{"type": "leave"}
{"type": "ping"}
```

Server → client:

```json
{"type": "joined", "you": "p2", "room": "kip-moza-tule", "peers": [{"id": "p1", "name": "Ada", "role": "host"}]}
{"type": "peer_joined", "peer": {"id": "p3", "name": "Lin", "role": "guest"}}
{"type": "peer_left", "id": "p3"}
{"type": "signal", "from": "p1", "payload": {...}}
{"type": "error", "code": "host_taken", "message": "this room already has a host"}
{"type": "pong"}
```

Error codes: `bad_message`, `bad_room`, `room_full`, `host_taken`,
`not_joined`, `unknown_peer`.

Room codes are 3 to 64 characters of `a-z`, `0-9` and `-`. Rooms exist while
someone is in them.

Other endpoints: `GET /health` → `ok`, `GET /rooms/new` → `{"room": "..."}`.

## Control channel (WebRTC data channel `control`)

Coordinates are `0..1` from the top-left of the shared display.

| Message | Direction | Notes |
| --- | --- | --- |
| `{"type":"hello","name":"Ada","os":"macos"}` | both | Sent when the channel opens. |
| `{"type":"request_control"}` | guest → host | |
| `{"type":"release_control"}` | guest → host | |
| `{"type":"grant_control","to":"p2"}` | host → guests | Broadcast to everyone. |
| `{"type":"revoke_control"}` | host → guests | Broadcast to everyone. |
| `{"type":"mouse_move","x":0.5,"y":0.25}` | controller → host | |
| `{"type":"mouse_down","x":0.5,"y":0.25,"button":"left"}` | controller → host | `left`, `right`, `middle` |
| `{"type":"mouse_up","x":0.5,"y":0.25,"button":"left"}` | controller → host | |
| `{"type":"scroll","dx":0,"dy":120}` | controller → host | Pixels. |
| `{"type":"key_down","code":"KeyC","modifiers":{"command":true}}` | controller → host | W3C `code`. `command` = platform shortcut key. |
| `{"type":"key_up","code":"KeyC","modifiers":{}}` | controller → host | |
| `{"type":"text","text":"é"}` | controller → host | Typed verbatim. |
| `{"type":"pointer","x":0.4,"y":0.6,"color":"#ff7a59"}` | guest → host → others | Shown on the host's screen. |
