// The viewer window: shows the host's screen over plain browser WebRTC,
// sends pointers, and drives the host's computer once allowed.
"use strict";

const ICE_SERVERS = [{ urls: "stun:stun.l.google.com:19302" }];
const POINTER_COLORS = ["#ff7a59", "#5c8cff", "#2fb36d", "#c05cff"];
const params = new URLSearchParams(location.search);
const ROOM = params.get("room") || "";
const SERVER = params.get("server") || DEFAULT_SERVER;
const NAME = params.get("name") || "Guest";

// ---------------------------------------------------------------- guest

const guest = {
  ws: null, pc: null, dc: null, mic: null, me: null, hostId: null, hostOs: "",
  mode: "point", controller: null, color: POINTER_COLORS[0], pendingIce: [],
};
const isMacGuest = /Mac/i.test(navigator.platform || navigator.userAgent);

async function joinRoom(server, room, name) {
  setStatus("Connecting…");
  $("stage-hint").hidden = false;

  try {
    guest.mic = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
    });
  } catch (e) {
    guest.mic = null;
    console.warn("no microphone", e);
  }

  const ws = new WebSocket(server);
  guest.ws = ws;
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = () => reject(new Error(`Can't reach ${server}`));
  });
  ws.onmessage = (e) => onSignal(JSON.parse(e.data)).catch((err) => console.error(err));
  ws.onclose = () => { if (guest.ws === ws) leave("Disconnected from the server."); };
  ws.send(JSON.stringify({ type: "join", room, name, role: "guest" }));
}

async function onSignal(msg) {
  switch (msg.type) {
    case "joined":
      guest.me = msg.you;
      guest.color = POINTER_COLORS[(parseInt(msg.you.replace(/\D/g, ""), 10) || 0) % POINTER_COLORS.length];
      $("my-pointer").style.background = guest.color;
      if (!msg.peers.some((p) => p.role === "host")) setStatus("Waiting for the host…");
      break;
    case "peer_joined":
      if (msg.peer.role === "host") setStatus("Host joined, connecting…");
      break;
    case "peer_left":
      if (msg.id === guest.hostId) {
        resetPeer();
        setStatus("Host left. Waiting…");
        $("stage-hint").hidden = false;
      }
      break;
    case "signal":
      await onPeerSignal(msg.from, msg.payload);
      break;
    case "error":
      leave(msg.message);
      break;
  }
}

async function onPeerSignal(from, payload) {
  if (payload.sdp?.type === "offer") {
    resetPeer();
    guest.hostId = from;
    const pc = new RTCPeerConnection({ iceServers: ICE_SERVERS });
    guest.pc = pc;
    if (guest.mic) for (const t of guest.mic.getTracks()) pc.addTrack(t, guest.mic);
    pc.onicecandidate = (e) => {
      if (e.candidate) sendSignal(from, { ice: e.candidate.toJSON() });
    };
    pc.ontrack = (e) => {
      if (e.track.kind === "video") {
        $("screen").srcObject = new MediaStream([e.track]);
        $("stage-hint").hidden = true;
      } else {
        $("voice").srcObject = new MediaStream([e.track]);
      }
    };
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === "connected") renderStatus();
      if (pc.connectionState === "failed") setStatus("Connection failed. A TURN server may be needed.");
    };
    pc.ondatachannel = (e) => setupChannel(e.channel);
    await pc.setRemoteDescription(payload.sdp);
    for (const c of guest.pendingIce.splice(0)) await pc.addIceCandidate(c).catch(() => {});
    const answer = await pc.createAnswer();
    await pc.setLocalDescription(answer);
    sendSignal(from, { sdp: { type: "answer", sdp: answer.sdp } });
  } else if (payload.ice) {
    if (guest.pc?.remoteDescription) await guest.pc.addIceCandidate(payload.ice).catch(() => {});
    else guest.pendingIce.push(payload.ice);
  }
}

function sendSignal(to, payload) {
  guest.ws?.send(JSON.stringify({ type: "signal", to, payload }));
}

function setupChannel(dc) {
  guest.dc = dc;
  dc.onopen = () => send({ type: "hello", name: NAME, os: isMacGuest ? "macos" : "other" });
  dc.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (m.type === "hello") guest.hostOs = m.os;
    else if (m.type === "grant_control") guest.controller = m.to;
    else if (m.type === "revoke_control") guest.controller = null;
    renderStatus();
  };
}

function send(msg) {
  if (guest.dc?.readyState === "open") guest.dc.send(JSON.stringify(msg));
}

const driving = () => guest.controller !== null && guest.controller === guest.me;

function renderStatus() {
  const pill = $("guest-status");
  if (driving()) {
    pill.textContent = "You're driving";
    pill.className = "pill driving";
  } else if (guest.pc?.connectionState === "connected") {
    pill.textContent = guest.mode === "control" ? "Asked for control…" : "Live";
    pill.className = "pill live";
  }
  $("stage").classList.toggle("controlling", driving());
}

function setStatus(text) {
  $("guest-status").textContent = text;
  $("guest-status").className = "pill";
}

function setMode(mode) {
  guest.mode = mode;
  $("mode-point").classList.toggle("active", mode === "point");
  $("mode-control").classList.toggle("active", mode === "control");
  $("my-pointer").hidden = true;
  if (mode === "control") {
    send({ type: "request_control" });
    $("stage").focus();
  } else if (driving()) {
    send({ type: "release_control" });
  }
  renderStatus();
}
$("mode-point").onclick = () => setMode("point");
$("mode-control").onclick = () => setMode("control");

$("guest-mute").onclick = () => {
  const track = guest.mic?.getAudioTracks()[0];
  if (!track) return;
  track.enabled = !track.enabled;
  $("guest-mute").textContent = track.enabled ? "Mute" : "Unmute";
};
$("guest-leave").onclick = () => leave();

function resetPeer() {
  guest.dc?.close();
  guest.pc?.close();
  guest.dc = guest.pc = null;
  guest.controller = null;
  guest.pendingIce = [];
  $("screen").srcObject = null;
}

function leave(reason) {
  const ws = guest.ws;
  guest.ws = null;
  if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: "leave" }));
  ws?.close();
  resetPeer();
  guest.mic?.getTracks().forEach((t) => t.stop());
  guest.mic = null;
  if (reason) {
    // Keep the window open so the reason can be read.
    setStatus("Disconnected");
    $("stage-hint").textContent = reason;
    $("stage-hint").hidden = false;
    $("guest-leave").textContent = "Close";
    $("guest-leave").onclick = closeWindow;
  } else {
    closeWindow();
  }
}

function closeWindow() {
  const w = tauri?.window?.getCurrentWindow?.();
  if (w) w.close().catch(() => window.close());
  else window.close();
}

// ---- input capture on the shared screen

/** Maps a mouse event to 0..1 coordinates on the shared screen, or null. */
function videoPoint(e) {
  const v = $("screen");
  if (!v.videoWidth) return null;
  const r = v.getBoundingClientRect();
  const scale = Math.min(r.width / v.videoWidth, r.height / v.videoHeight);
  const w = v.videoWidth * scale, h = v.videoHeight * scale;
  const left = r.left + (r.width - w) / 2, top = r.top + (r.height - h) / 2;
  const x = (e.clientX - left) / w, y = (e.clientY - top) / h;
  if (x < 0 || y < 0 || x > 1 || y > 1) return null;
  return { x, y, px: e.clientX - r.left, py: e.clientY - r.top };
}

let moveQueued = null;
function flushMove() {
  const p = moveQueued;
  moveQueued = null;
  if (!p) return;
  if (driving()) send({ type: "mouse_move", x: p.x, y: p.y });
  else if (guest.mode === "point") send({ type: "pointer", x: p.x, y: p.y, color: guest.color });
}

const stage = $("stage");
stage.addEventListener("mousemove", (e) => {
  const p = videoPoint(e);
  const dot = $("my-pointer");
  dot.hidden = !(p && guest.mode === "point");
  if (!p) return;
  dot.style.left = p.px + "px";
  dot.style.top = p.py + "px";
  if (!moveQueued) requestAnimationFrame(flushMove);
  moveQueued = p;
});
stage.addEventListener("mouseleave", () => { $("my-pointer").hidden = true; });

const BUTTONS = ["left", "middle", "right"];
for (const type of ["mousedown", "mouseup"]) {
  stage.addEventListener(type, (e) => {
    if (!driving()) return;
    const p = videoPoint(e);
    if (!p) return;
    e.preventDefault();
    stage.focus();
    send({ type: type === "mousedown" ? "mouse_down" : "mouse_up", x: p.x, y: p.y, button: BUTTONS[e.button] ?? "left" });
  });
}
stage.addEventListener("contextmenu", (e) => { if (driving()) e.preventDefault(); });
stage.addEventListener("wheel", (e) => {
  if (!driving()) return;
  e.preventDefault();
  const k = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? 800 : 1;
  send({ type: "scroll", dx: e.deltaX * k, dy: e.deltaY * k });
}, { passive: false });

/** Cmd on a Mac guest and Ctrl elsewhere both mean "the shortcut key". */
function modifiers(e) {
  return {
    shift: e.shiftKey,
    option: e.altKey,
    command: isMacGuest ? e.metaKey : e.ctrlKey,
    control: isMacGuest ? e.ctrlKey : false,
  };
}

const isModifierCode = (code) => /^(Shift|Control|Alt|Meta)(Left|Right)$/.test(code);

stage.addEventListener("keydown", (e) => {
  if (!driving()) return;
  e.preventDefault();
  if (isModifierCode(e.code)) return; // sent as flags on the next key instead
  const m = modifiers(e);
  const printable = e.key.length === 1 && !m.command && !m.control && !m.option;
  if (printable) send({ type: "text", text: e.key });
  else send({ type: "key_down", code: e.code, modifiers: m });
});
stage.addEventListener("keyup", (e) => {
  if (!driving()) return;
  e.preventDefault();
  if (isModifierCode(e.code)) return;
  send({ type: "key_up", code: e.code, modifiers: modifiers(e) });
});

document.title = `Duet · ${ROOM}`;
$("room-label").textContent = ROOM;
window.addEventListener("beforeunload", () => guest.ws?.send(JSON.stringify({ type: "leave" })));
joinRoom(SERVER, ROOM, NAME).catch((e) => leave(errorText(e)));
