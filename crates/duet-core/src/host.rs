//! A hosting session: one shared screen, many guests.
//!
//! The host is the WebRTC offerer for every guest. Each guest gets its own
//! peer connection carrying the shared screen track, the host's microphone,
//! and a `control` data channel.

use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use duet_protocol::{
    ClientMsg, ControlEvent, ControlMsg, ControlState, PeerId, PeerInfo, Role, ServerMsg,
    CONTROL_CHANNEL,
};
use futures::{SinkExt, StreamExt};
use libwebrtc::data_channel::{DataChannel, DataChannelInit, DataChannelState};
use libwebrtc::ice_candidate::IceCandidate;
use libwebrtc::media_stream_track::MediaStreamTrack;
use libwebrtc::peer_connection::{OfferOptions, PeerConnection, PeerConnectionState};
use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
use libwebrtc::peer_connection_factory::{IceServer, PeerConnectionFactory, RtcConfiguration};
use libwebrtc::rtp_parameters::DegradationPreference;
use libwebrtc::session_description::{SdpType, SessionDescription};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::capture::{ScreenCapture, DEFAULT_FPS};
use crate::input::Injector;

pub const DEFAULT_STUN: &str = "stun:stun.l.google.com:19302";
const MAX_VIDEO_BITRATE: u64 = 8_000_000;

#[derive(Debug, Clone)]
pub struct HostConfig {
    /// e.g. `ws://192.168.1.20:8787/ws`
    pub server_url: String,
    pub name: String,
    /// Room to host; a fresh code is generated when `None`.
    pub room: Option<String>,
    pub ice_servers: Vec<String>,
    pub fps: u32,
}

impl HostConfig {
    pub fn new(server_url: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            server_url: server_url.into(),
            name: name.into(),
            room: None,
            ice_servers: vec![DEFAULT_STUN.into()],
            fps: DEFAULT_FPS,
        }
    }
}

/// Things the host UI shows. Serialized as `{"type": "...", ...}`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostEvent {
    Started { room: String },
    PeerJoined { peer: PeerInfo },
    PeerLeft { id: PeerId },
    PeerConnection { id: PeerId, state: String },
    ControlRequested { id: PeerId },
    ControlChanged { controller: Option<PeerId> },
    Pointer { id: PeerId, x: f64, y: f64, color: String },
    Error { message: String },
    Stopped,
}

#[derive(Debug)]
enum Command {
    Grant(PeerId),
    Deny(PeerId),
    Revoke,
    SetAutoGrant(bool),
    SetMuted(bool),
    Stop,
}

/// Cheap, cloneable remote for a running session.
#[derive(Clone)]
pub struct HostHandle {
    tx: mpsc::UnboundedSender<Command>,
    pub room: String,
}

impl HostHandle {
    pub fn grant(&self, peer: PeerId) {
        let _ = self.tx.send(Command::Grant(peer));
    }
    pub fn deny(&self, peer: PeerId) {
        let _ = self.tx.send(Command::Deny(peer));
    }
    pub fn revoke(&self) {
        let _ = self.tx.send(Command::Revoke);
    }
    pub fn set_auto_grant(&self, on: bool) {
        let _ = self.tx.send(Command::SetAutoGrant(on));
    }
    pub fn set_muted(&self, muted: bool) {
        let _ = self.tx.send(Command::SetMuted(muted));
    }
    pub fn stop(&self) {
        let _ = self.tx.send(Command::Stop);
    }
}

/// Events raised on WebRTC and input threads, funnelled into the session loop.
enum Internal {
    Ice(PeerId, Value),
    Channel(PeerId, DataChannelState),
    Data(PeerId, String),
    State(PeerId, PeerConnectionState),
    HostTookMouse,
}

enum InputCmd {
    Apply(ControlMsg),
    Active(bool),
}

struct Guest {
    pc: PeerConnection,
    dc: DataChannel,
}

/// Connects to the signaling server, claims the room as host and starts
/// sharing. Returns once the room is joined.
pub async fn start(
    cfg: HostConfig,
    events: mpsc::UnboundedSender<HostEvent>,
) -> anyhow::Result<HostHandle> {
    let room = cfg.room.clone().unwrap_or_else(duet_protocol::new_room_code);
    let (mut ws, _) = tokio_tungstenite::connect_async(&cfg.server_url).await?;
    let join = ClientMsg::Join { room: room.clone(), name: cfg.name.clone(), role: Role::Host };
    ws.send(Message::text(serde_json::to_string(&join)?)).await?;

    let existing = loop {
        let Some(frame) = ws.next().await else { anyhow::bail!("server closed the connection") };
        let Message::Text(text) = frame? else { continue };
        match serde_json::from_str::<ServerMsg>(&text)? {
            ServerMsg::Joined { peers, .. } => break peers,
            ServerMsg::Error { message, .. } => anyhow::bail!(message),
            _ => continue,
        }
    };

    let capture = ScreenCapture::start(cfg.fps)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = HostHandle { tx, room: room.clone() };
    let _ = events.send(HostEvent::Started { room });

    tokio::spawn(async move {
        let mut session = match Session::new(cfg, capture, events.clone()) {
            Ok(s) => s,
            Err(e) => {
                let _ = events.send(HostEvent::Error { message: e.to_string() });
                let _ = events.send(HostEvent::Stopped);
                return;
            }
        };
        if let Err(e) = session.run(ws, rx, existing).await {
            let _ = events.send(HostEvent::Error { message: e.to_string() });
        }
        session.shutdown();
        let _ = events.send(HostEvent::Stopped);
    });

    Ok(handle)
}

struct Session {
    cfg: HostConfig,
    factory: PeerConnectionFactory,
    capture: ScreenCapture,
    mic: libwebrtc::audio_track::RtcAudioTrack,
    guests: HashMap<PeerId, Guest>,
    control: ControlState,
    events: mpsc::UnboundedSender<HostEvent>,
    internal_tx: mpsc::UnboundedSender<Internal>,
    internal_rx: mpsc::UnboundedReceiver<Internal>,
    input_tx: std_mpsc::Sender<InputCmd>,
    outbox: Vec<ClientMsg>,
}

impl Session {
    fn new(
        cfg: HostConfig,
        capture: ScreenCapture,
        events: mpsc::UnboundedSender<HostEvent>,
    ) -> anyhow::Result<Self> {
        let factory = PeerConnectionFactory::default();
        factory.acquire_platform_adm();
        factory.set_adm_playout_enabled(true);
        let mic = factory.create_device_audio_track("duet-mic");

        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        let input_tx = spawn_input_thread(internal_tx.clone(), events.clone());

        Ok(Self {
            cfg,
            factory,
            capture,
            mic,
            guests: HashMap::new(),
            control: ControlState::default(),
            events,
            internal_tx,
            internal_rx,
            input_tx,
            outbox: Vec::new(),
        })
    }

    async fn run<S>(
        &mut self,
        ws: S,
        mut commands: mpsc::UnboundedReceiver<Command>,
        existing: Vec<PeerInfo>,
    ) -> anyhow::Result<()>
    where
        S: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
            + Unpin,
    {
        let (mut sink, mut stream) = ws.split();
        for peer in existing {
            self.add_guest(peer).await;
        }
        let mut ping = tokio::time::interval(Duration::from_secs(20));

        loop {
            for msg in self.outbox.drain(..) {
                sink.send(Message::text(serde_json::to_string(&msg)?)).await?;
            }
            tokio::select! {
                frame = stream.next() => {
                    let Some(frame) = frame else { anyhow::bail!("signaling server disconnected") };
                    if let Message::Text(text) = frame? {
                        match serde_json::from_str::<ServerMsg>(&text) {
                            Ok(msg) => self.on_server(msg).await,
                            Err(e) => tracing::warn!("bad server message: {e}"),
                        }
                    }
                }
                Some(ev) = self.internal_rx.recv() => self.on_internal(ev),
                cmd = commands.recv() => match cmd {
                    None | Some(Command::Stop) => {
                        let _ = sink.send(Message::text(serde_json::to_string(&ClientMsg::Leave)?)).await;
                        return Ok(());
                    }
                    Some(cmd) => self.on_command(cmd),
                },
                _ = ping.tick() => self.outbox.push(ClientMsg::Ping),
            }
        }
    }

    async fn on_server(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::PeerJoined { peer } => self.add_guest(peer).await,
            ServerMsg::PeerLeft { id } => self.remove_guest(&id),
            ServerMsg::Signal { from, payload } => {
                if let Err(e) = self.on_signal(&from, payload).await {
                    tracing::warn!(peer = from, "signal failed: {e}");
                }
            }
            ServerMsg::Error { message, .. } => {
                let _ = self.events.send(HostEvent::Error { message });
            }
            ServerMsg::Joined { .. } | ServerMsg::Pong => {}
        }
    }

    async fn add_guest(&mut self, peer: PeerInfo) {
        if peer.role != Role::Guest {
            return;
        }
        let id = peer.id.clone();
        let _ = self.events.send(HostEvent::PeerJoined { peer });
        match self.connect_guest(&id).await {
            Ok(guest) => {
                self.guests.insert(id, guest);
            }
            Err(e) => {
                let _ = self.events.send(HostEvent::Error { message: format!("connecting {id}: {e}") });
            }
        }
    }

    async fn connect_guest(&mut self, id: &PeerId) -> anyhow::Result<Guest> {
        let mut config = RtcConfiguration::default();
        config.ice_servers = vec![IceServer {
            urls: self.cfg.ice_servers.clone(),
            username: String::new(),
            password: String::new(),
        }];
        let pc = self.factory.create_peer_connection(config)?;

        let tx = self.internal_tx.clone();
        let pid = id.clone();
        pc.on_ice_candidate(Some(Box::new(move |c: IceCandidate| {
            let ice = json!({
                "candidate": c.candidate(),
                "sdpMid": c.sdp_mid(),
                "sdpMLineIndex": c.sdp_mline_index(),
            });
            let _ = tx.send(Internal::Ice(pid.clone(), ice));
        })));
        let tx = self.internal_tx.clone();
        let pid = id.clone();
        pc.on_connection_state_change(Some(Box::new(move |s| {
            let _ = tx.send(Internal::State(pid.clone(), s));
        })));

        let video = self.factory.create_video_track("duet-screen", self.capture.source());
        let sender = pc.add_track(MediaStreamTrack::from(video), &["duet"])?;
        let mut params = sender.parameters();
        params.set_degradation_preference(DegradationPreference::MaintainResolution);
        for enc in &mut params.encodings {
            enc.max_bitrate = Some(MAX_VIDEO_BITRATE);
            enc.max_framerate = Some(self.cfg.fps as f64);
        }
        if let Err(e) = sender.set_parameters(params) {
            tracing::warn!("set_parameters: {e:?}");
        }
        pc.add_track(MediaStreamTrack::from(self.mic.clone()), &["duet"])?;

        let dc = pc.create_data_channel(CONTROL_CHANNEL, DataChannelInit::default())?;
        let tx = self.internal_tx.clone();
        let pid = id.clone();
        dc.on_state_change(Some(Box::new(move |s| {
            let _ = tx.send(Internal::Channel(pid.clone(), s));
        })));
        let tx = self.internal_tx.clone();
        let pid = id.clone();
        dc.on_message(Some(Box::new(move |buf| {
            if let Ok(text) = std::str::from_utf8(buf.data) {
                let _ = tx.send(Internal::Data(pid.clone(), text.to_owned()));
            }
        })));

        let offer = pc.create_offer(OfferOptions::default()).await?;
        pc.set_local_description(offer.clone()).await?;
        self.outbox.push(ClientMsg::Signal {
            to: id.clone(),
            payload: json!({ "sdp": { "type": "offer", "sdp": offer.to_string() } }),
        });
        Ok(Guest { pc, dc })
    }

    async fn on_signal(&mut self, from: &PeerId, payload: Value) -> anyhow::Result<()> {
        let Some(guest) = self.guests.get(from) else { return Ok(()) };
        if let Some(sdp) = payload.get("sdp") {
            let kind = sdp["type"].as_str().unwrap_or_default();
            anyhow::ensure!(kind == "answer", "host only accepts answers, got {kind:?}");
            let desc = SessionDescription::parse(sdp["sdp"].as_str().unwrap_or_default(), SdpType::Answer)?;
            guest.pc.set_remote_description(desc).await?;
        } else if let Some(ice) = payload.get("ice") {
            let cand = ice["candidate"].as_str().unwrap_or_default();
            if cand.is_empty() {
                return Ok(()); // end-of-candidates marker
            }
            let c = IceCandidate::parse(
                ice["sdpMid"].as_str().unwrap_or_default(),
                ice["sdpMLineIndex"].as_i64().unwrap_or(0) as i32,
                cand,
            )?;
            guest.pc.add_ice_candidate(c).await?;
        }
        Ok(())
    }

    fn on_internal(&mut self, ev: Internal) {
        match ev {
            Internal::Ice(to, ice) => {
                self.outbox.push(ClientMsg::Signal { to, payload: json!({ "ice": ice }) })
            }
            Internal::Channel(id, DataChannelState::Open) => {
                let os = std::env::consts::OS.to_string();
                self.send_to(&id, &ControlMsg::Hello { name: self.cfg.name.clone(), os });
                if let Some(c) = self.control.controller().cloned() {
                    self.send_to(&id, &ControlMsg::GrantControl { to: c });
                }
            }
            Internal::Channel(..) => {}
            Internal::State(id, state) => {
                let _ = self.events.send(HostEvent::PeerConnection { id, state: format!("{state:?}").to_lowercase() });
            }
            Internal::Data(from, text) => match serde_json::from_str::<ControlMsg>(&text) {
                Ok(msg) => self.on_control(from, msg),
                Err(e) => tracing::debug!(peer = from, "bad control message: {e}"),
            },
            Internal::HostTookMouse => {
                if let Some(ev) = self.control.revoke() {
                    self.apply_control_event(ev);
                }
            }
        }
    }

    fn on_control(&mut self, from: PeerId, msg: ControlMsg) {
        if self.control.accepts_input(&from, &msg) {
            let _ = self.input_tx.send(InputCmd::Apply(msg));
            return;
        }
        match msg {
            ControlMsg::RequestControl | ControlMsg::ReleaseControl => {
                if let Some(ev) = self.control.on_peer_msg(&from, &msg) {
                    self.apply_control_event(ev);
                }
            }
            ControlMsg::Pointer { x, y, color } => {
                let fwd = ControlMsg::Pointer { x, y, color: color.clone() }.clamped();
                if let ControlMsg::Pointer { x, y, .. } = fwd {
                    let _ = self.events.send(HostEvent::Pointer { id: from.clone(), x, y, color });
                }
                for id in self.guests.keys().filter(|id| **id != from).cloned().collect::<Vec<_>>() {
                    self.send_to(&id, &fwd);
                }
            }
            _ => {}
        }
    }

    fn on_command(&mut self, cmd: Command) {
        let ev = match cmd {
            Command::Grant(peer) if self.guests.contains_key(&peer) => Some(self.control.grant(peer)),
            Command::Grant(_) => None,
            Command::Deny(peer) => {
                self.control.deny(&peer);
                None
            }
            Command::Revoke => self.control.revoke(),
            Command::SetAutoGrant(on) => {
                self.control.auto_grant = on;
                None
            }
            Command::SetMuted(muted) => {
                self.mic.set_enabled(!muted);
                None
            }
            Command::Stop => None,
        };
        if let Some(ev) = ev {
            self.apply_control_event(ev);
        }
    }

    fn apply_control_event(&mut self, ev: ControlEvent) {
        match ev {
            ControlEvent::Prompt(id) => {
                let _ = self.events.send(HostEvent::ControlRequested { id });
            }
            ControlEvent::Granted(id) => {
                let _ = self.input_tx.send(InputCmd::Active(true));
                self.broadcast(&ControlMsg::GrantControl { to: id.clone() });
                let _ = self.events.send(HostEvent::ControlChanged { controller: Some(id) });
            }
            ControlEvent::Revoked(_) => {
                let _ = self.input_tx.send(InputCmd::Active(false));
                self.broadcast(&ControlMsg::RevokeControl);
                let _ = self.events.send(HostEvent::ControlChanged { controller: None });
            }
        }
    }

    fn remove_guest(&mut self, id: &PeerId) {
        if let Some(g) = self.guests.remove(id) {
            g.dc.close();
            g.pc.close();
        }
        if let Some(ev) = self.control.on_peer_left(id) {
            self.apply_control_event(ev);
        }
        let _ = self.events.send(HostEvent::PeerLeft { id: id.clone() });
    }

    fn send_to(&self, id: &PeerId, msg: &ControlMsg) {
        if let Some(g) = self.guests.get(id) {
            if g.dc.state() == DataChannelState::Open {
                let text = serde_json::to_string(msg).expect("ControlMsg serializes");
                let _ = g.dc.send(text.as_bytes(), false);
            }
        }
    }

    fn broadcast(&self, msg: &ControlMsg) {
        for id in self.guests.keys() {
            self.send_to(id, msg);
        }
    }

    fn shutdown(&mut self) {
        let _ = self.input_tx.send(InputCmd::Active(false));
        for (_, g) in self.guests.drain() {
            g.dc.close();
            g.pc.close();
        }
        self.factory.release_platform_adm();
    }
}

/// Input injection lives on its own thread: enigo handles aren't `Send` on
/// every platform, and injection must never stall the session loop.
fn spawn_input_thread(
    internal: mpsc::UnboundedSender<Internal>,
    events: mpsc::UnboundedSender<HostEvent>,
) -> std_mpsc::Sender<InputCmd> {
    let (tx, rx) = std_mpsc::channel::<InputCmd>();
    std::thread::Builder::new()
        .name("duet-input".into())
        .spawn(move || {
            let mut injector: Option<Injector> = None;
            let mut active = false;
            loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(InputCmd::Active(on)) => {
                        active = on;
                        if let Some(inj) = &mut injector {
                            inj.forget_position();
                        }
                    }
                    Ok(InputCmd::Apply(msg)) if active => {
                        if injector.is_none() {
                            match Injector::new() {
                                Ok(i) => injector = Some(i),
                                Err(e) => {
                                    let _ = events.send(HostEvent::Error {
                                        message: format!("remote control unavailable: {e} (on macOS, allow Duet under Privacy & Security → Accessibility)"),
                                    });
                                    let _ = internal.send(Internal::HostTookMouse);
                                    active = false;
                                    continue;
                                }
                            }
                        }
                        if let Some(inj) = &mut injector {
                            if let Err(e) = inj.apply(&msg) {
                                tracing::warn!("inject failed: {e}");
                            }
                        }
                    }
                    Ok(InputCmd::Apply(_)) => {}
                    Err(std_mpsc::RecvTimeoutError::Timeout) => {
                        if active && injector.as_ref().is_some_and(|i| i.host_moved_mouse()) {
                            active = false;
                            let _ = internal.send(Internal::HostTookMouse);
                        }
                    }
                    Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("spawn input thread");
    tx
}
