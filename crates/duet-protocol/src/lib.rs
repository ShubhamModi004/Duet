//! Duet wire protocol.
//!
//! Two channels carry JSON messages:
//!
//! * **Signaling** (WebSocket to `duet-signal`): joining a room and relaying
//!   WebRTC offers/answers/ICE candidates between peers. See [`ClientMsg`] and
//!   [`ServerMsg`].
//! * **Control** (WebRTC data channel labelled `"control"`, peer to peer): remote
//!   input, control hand-off and pointers. See [`ControlMsg`].
//!
//! Every message is an internally tagged JSON object: `{"type": "...", ...}`.
//! The Swift (macOS) and Dart (mobile) clients mirror these shapes; the
//! canonical examples live in `docs/PROTOCOL.md`.

use rand::Rng;
use serde::{Deserialize, Serialize};

pub mod control;

pub use control::{ControlEvent, ControlState};

/// Data channel label used for [`ControlMsg`] traffic.
pub const CONTROL_CHANNEL: &str = "control";

/// Max peers in one room. Duet is a pairing tool: one host plus a few guests.
pub const MAX_ROOM_PEERS: usize = 4;

pub type PeerId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Shares their screen and may accept remote control (macOS only).
    Host,
    /// Views, talks, points, and may control the host when granted.
    Guest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: PeerId,
    pub name: String,
    pub role: Role,
}

/// Client → signaling server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Join { room: String, name: String, role: Role },
    /// Opaque WebRTC payload (SDP or ICE) for one peer.
    Signal { to: PeerId, payload: serde_json::Value },
    Leave,
    Ping,
}

/// Signaling server → client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Joined { you: PeerId, room: String, peers: Vec<PeerInfo> },
    PeerJoined { peer: PeerInfo },
    PeerLeft { id: PeerId },
    Signal { from: PeerId, payload: serde_json::Value },
    Error { code: ErrorCode, message: String },
    Pong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadMessage,
    BadRoom,
    RoomFull,
    HostTaken,
    NotJoined,
    UnknownPeer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Peer ↔ peer over the `control` data channel.
///
/// Coordinates are normalized to the shared display: `0.0..=1.0` from the
/// top-left corner, so guests never need to know the host's resolution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMsg {
    // Control hand-off.
    RequestControl,
    GrantControl { to: PeerId },
    RevokeControl,
    ReleaseControl,

    // Remote input (only honoured from the peer that holds control).
    MouseMove { x: f64, y: f64 },
    MouseDown { x: f64, y: f64, button: MouseButton },
    MouseUp { x: f64, y: f64, button: MouseButton },
    Scroll { dx: f64, dy: f64 },
    /// `code` is a W3C `KeyboardEvent.code` value such as `"KeyA"` or `"Enter"`.
    KeyDown { code: String, modifiers: Modifiers },
    KeyUp { code: String, modifiers: Modifiers },
    /// Committed text from a soft keyboard or IME; typed verbatim.
    Text { text: String },

    // Presence. The host fills `from` and `name` when it relays a guest's
    // pointer or stroke to everyone else; guests leave them empty.
    Pointer {
        x: f64,
        y: f64,
        color: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<PeerId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Part of a freehand stroke. A stroke arrives as several messages with the
    /// same `stroke` id, each carrying only its new points; `end` closes it.
    Draw {
        stroke: String,
        points: Vec<[f64; 2]>,
        color: String,
        #[serde(default)]
        end: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<PeerId>,
    },
    ClearDrawings,
    HostCursor { x: f64, y: f64 },
    /// First message each side sends. `os` is `"macos"`, `"windows"`, ... so a
    /// guest can map Ctrl/Cmd shortcuts to the host's platform.
    Hello {
        name: String,
        #[serde(default)]
        os: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifiers {
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub control: bool,
    #[serde(default)]
    pub option: bool,
    #[serde(default)]
    pub command: bool,
}

impl ControlMsg {
    /// True for messages that move the host's mouse or keyboard.
    pub fn is_input(&self) -> bool {
        matches!(
            self,
            ControlMsg::MouseMove { .. }
                | ControlMsg::MouseDown { .. }
                | ControlMsg::MouseUp { .. }
                | ControlMsg::Scroll { .. }
                | ControlMsg::KeyDown { .. }
                | ControlMsg::KeyUp { .. }
                | ControlMsg::Text { .. }
        )
    }

    /// Clamp coordinates into the unit square so a buggy or hostile peer can't
    /// drive the cursor off-screen.
    pub fn clamped(mut self) -> Self {
        fn c(v: &mut f64) {
            *v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        }
        match &mut self {
            ControlMsg::Draw { points, .. } => {
                for [x, y] in points.iter_mut() {
                    c(x);
                    c(y);
                }
            }
            ControlMsg::MouseMove { x, y }
            | ControlMsg::MouseDown { x, y, .. }
            | ControlMsg::MouseUp { x, y, .. }
            | ControlMsg::Pointer { x, y, .. }
            | ControlMsg::HostCursor { x, y } => {
                c(x);
                c(y);
            }
            _ => {}
        }
        self
    }
}

/// Room codes look like `kip-moza-tule`: easy to read aloud, ~37 bits.
pub fn new_room_code() -> String {
    const C: &[u8] = b"bdfghjklmnprstvz";
    const V: &[u8] = b"aeiou";
    let mut rng = rand::thread_rng();
    let mut word = |syllables: usize| {
        let mut w = String::new();
        for _ in 0..syllables {
            w.push(C[rng.gen_range(0..C.len())] as char);
            w.push(V[rng.gen_range(0..V.len())] as char);
        }
        w
    };
    format!("{}-{}-{}", word(2), word(2), word(2))
}

/// Accepts lowercase ASCII letters, digits and `-`, 3..=64 chars.
pub fn is_valid_room_code(code: &str) -> bool {
    (3..=64).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn client_msgs_use_snake_case_tags() {
        let m: ClientMsg = serde_json::from_value(json!({
            "type": "join", "room": "kip-moza-tule", "name": "Ana", "role": "guest"
        }))
        .unwrap();
        assert_eq!(
            m,
            ClientMsg::Join { room: "kip-moza-tule".into(), name: "Ana".into(), role: Role::Guest }
        );
    }

    #[test]
    fn control_msgs_round_trip() {
        let m = ControlMsg::KeyDown {
            code: "KeyA".into(),
            modifiers: Modifiers { command: true, ..Default::default() },
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["type"], "key_down");
        assert_eq!(v["modifiers"]["command"], true);
        assert_eq!(serde_json::from_value::<ControlMsg>(v).unwrap(), m);
    }

    #[test]
    fn modifiers_default_when_omitted() {
        let m: ControlMsg =
            serde_json::from_value(json!({"type": "key_up", "code": "Enter", "modifiers": {}}))
                .unwrap();
        assert_eq!(m, ControlMsg::KeyUp { code: "Enter".into(), modifiers: Modifiers::default() });
    }

    #[test]
    fn clamps_coordinates() {
        let m = ControlMsg::MouseMove { x: 1.7, y: f64::NAN }.clamped();
        assert_eq!(m, ControlMsg::MouseMove { x: 1.0, y: 0.0 });
    }

    #[test]
    fn pointer_and_draw_wire_format() {
        let p: ControlMsg = serde_json::from_value(json!({
            "type": "pointer", "x": 0.2, "y": 0.3, "color": "#f00"
        }))
        .unwrap();
        assert!(matches!(p, ControlMsg::Pointer { from: None, name: None, .. }));
        // Relayed pointers carry who sent them; local ones don't serialize nulls.
        assert!(serde_json::to_value(&p).unwrap().get("from").is_none());

        let d = ControlMsg::Draw {
            stroke: "s1".into(),
            points: vec![[1.5, 0.5], [-1.0, f64::INFINITY]],
            color: "#0f0".into(),
            end: false,
            from: None,
        }
        .clamped();
        let ControlMsg::Draw { points, .. } = d else { unreachable!() };
        assert_eq!(points, vec![[1.0, 0.5], [0.0, 0.0]]);
        assert!(!ControlMsg::ClearDrawings.is_input());
    }

    #[test]
    fn room_codes_are_valid() {
        for _ in 0..100 {
            let c = new_room_code();
            assert!(is_valid_room_code(&c), "{c}");
            assert_eq!(c.len(), 14);
        }
        assert!(!is_valid_room_code("Bad Room"));
        assert!(!is_valid_room_code("ab"));
    }
}
