// The menu-bar panel: start/stop sharing, join a pair, settings.
"use strict";

function show(id) {
  for (const v of document.querySelectorAll(".view")) v.hidden = v.id !== id;
}

function myName() {
  return prefs.get("name").trim() || "Guest";
}

/** The server URL to use, or null for the built-in server on this computer. */
function hostServer() {
  if (prefs.get("hostLocal") === "1") return null;
  return normalizeServer(prefs.get("server")) || DEFAULT_SERVER;
}

function joinServer() {
  return normalizeServer(prefs.get("server")) || DEFAULT_SERVER;
}

function homeError(e) {
  $("home-error").textContent = e ? errorText(e) : "";
}

function renderFooter() {
  $("who").textContent = myName();
  const s = prefs.get("server").trim();
  $("server-label").textContent = prefs.get("hostLocal") === "1" ? "Same network" : s ? s.replace(/^wss?:\/\//, "").replace(/\/ws$/, "") : "";
}

// ---------------------------------------------------------------- settings

$("name").value = prefs.get("name");
$("server").value = prefs.get("server");
$("host-local").checked = prefs.get("hostLocal") === "1";
$("name").oninput = (e) => prefs.set("name", e.target.value.trim());
$("server").oninput = (e) => prefs.set("server", e.target.value.trim());
$("host-local").onchange = (e) => prefs.set("hostLocal", e.target.checked ? "1" : "0");
$("open-settings").onclick = () => show("settings");
$("close-settings").onclick = () => { renderFooter(); show("home"); };
$("quit").onclick = () => invoke("quit");

// First run: ask for a name so guests see who they're pairing with.
renderFooter();
if (!prefs.get("name")) {
  show("settings");
  $("name").focus();
}

// ---------------------------------------------------------------- join

$("join-form").onsubmit = async (e) => {
  e.preventDefault();
  homeError();
  const room = $("join-room").value.trim().toLowerCase();
  if (!room) return homeError("Enter the room code your pair sent you.");
  try {
    await invoke("open_viewer", { room, server: joinServer(), name: myName() });
    $("join-room").value = "";
  } catch (err) {
    homeError(err);
  }
};

// ---------------------------------------------------------------- host

const host = { guests: new Map(), controller: null, muted: false, unlisten: null };

$("host-btn").onclick = () => startHosting().catch(homeError);

async function startHosting() {
  homeError();
  const btn = $("host-btn");
  btn.disabled = true;
  btn.querySelector(".big-title").textContent = "Starting…";
  btn.querySelector(".big-sub").textContent = "If macOS asks for Screen Recording, allow it";
  // Keep the panel open while macOS shows permission prompts.
  await invoke("set_panel_pinned", { pinned: true }).catch(() => {});
  try {
    host.unlisten = await tauri.event.listen("host-event", (e) => onHostEvent(e.payload));
    const server = hostServer();
    const res = await invoke("host_start", { name: myName(), server });
    $("room-code").textContent = res.room;
    const hint = $("server-hint");
    hint.hidden = server === DEFAULT_SERVER;
    hint.textContent = server === null
      ? "Server: " + (res.join_servers[0] ?? "no network address found")
      : "Server: " + server;
    host.guests.clear();
    host.controller = null;
    $("host-log").innerHTML = "";
    renderGuests();
    show("live");
  } catch (e) {
    host.unlisten?.();
    host.unlisten = null;
    throw e;
  } finally {
    invoke("set_panel_pinned", { pinned: false }).catch(() => {});
    btn.disabled = false;
    btn.querySelector(".big-title").textContent = "Start sharing";
    btn.querySelector(".big-sub").textContent = "Your pair sees your screen and can drive when you allow it";
  }
}

function onHostEvent(ev) {
  switch (ev.type) {
    case "peer_joined":
      host.guests.set(ev.peer.id, { name: ev.peer.name, state: "new", wants: false });
      log(`${ev.peer.name} joined`);
      break;
    case "peer_left": {
      const g = host.guests.get(ev.id);
      host.guests.delete(ev.id);
      if (g) log(`${g.name} left`);
      break;
    }
    case "peer_connection": {
      const g = host.guests.get(ev.id);
      if (g) g.state = ev.state;
      break;
    }
    case "control_requested": {
      const g = host.guests.get(ev.id);
      if (g) g.wants = true;
      break;
    }
    case "control_changed":
      host.controller = ev.controller;
      for (const g of host.guests.values()) g.wants = false;
      break;
    case "error":
      log("⚠ " + ev.message);
      break;
    case "stopped":
      host.unlisten?.();
      host.unlisten = null;
      show("home");
      return;
    default:
      return; // pointers and drawings are shown by the overlay window
  }
  renderGuests();
}

function renderGuests() {
  const ul = $("guests");
  ul.innerHTML = "";
  if (!host.guests.size) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = "Waiting for someone to join…";
    ul.append(li);
  }
  for (const [id, g] of host.guests) {
    const li = document.createElement("li");
    const avatar = document.createElement("span");
    avatar.className = "avatar" + (g.state === "connected" ? " on" : "");
    avatar.textContent = (g.name[0] || "?").toUpperCase();
    const text = document.createElement("span");
    text.className = "who";
    text.textContent = g.name;
    const sub = document.createElement("small");
    sub.textContent = host.controller === id ? "Driving" : g.wants ? "Wants control" : g.state === "connected" ? "Watching" : "Connecting…";
    text.append(sub);
    li.append(avatar, text);
    if (g.wants) {
      li.classList.add("asking");
      li.append(
        button("Deny", "ghost small", () => { g.wants = false; invoke("host_deny", { peer: id }); renderGuests(); }),
        button("Allow", "primary small", () => invoke("host_grant", { peer: id })),
      );
    } else if (host.controller !== id && g.state === "connected") {
      li.append(button("Give control", "ghost small", () => invoke("host_grant", { peer: id })));
    }
    ul.append(li);
  }
  const driving = host.controller && host.guests.get(host.controller);
  $("control-banner").hidden = !driving;
  if (driving) $("control-text").textContent = `${driving.name} is controlling your computer`;
}

function button(text, cls, onclick) {
  const b = document.createElement("button");
  b.textContent = text;
  b.className = cls;
  b.onclick = onclick;
  return b;
}

function log(text) {
  const li = document.createElement("li");
  li.textContent = new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) + "  " + text;
  $("host-log").prepend(li);
  while ($("host-log").children.length > 20) $("host-log").lastChild.remove();
}

$("copy-room").onclick = async () => {
  try {
    await navigator.clipboard.writeText($("room-code").textContent);
    $("copy-room").textContent = "Copied";
    setTimeout(() => ($("copy-room").textContent = "Copy"), 1200);
  } catch {}
};
$("host-stop").onclick = () => invoke("host_stop");
$("take-back").onclick = () => invoke("host_revoke");
$("auto-grant").onchange = (e) => invoke("host_set_auto_grant", { on: e.target.checked });
$("host-mute").onclick = () => {
  host.muted = !host.muted;
  $("host-mute").textContent = host.muted ? "Unmute" : "Mute";
  invoke("host_set_muted", { muted: host.muted });
};

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !$("settings").hidden) $("close-settings").click();
});
