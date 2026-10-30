// Shared by the menu-bar panel (index.html) and the viewer window (viewer.html).
"use strict";

const tauri = window.__TAURI__;
const invoke = (cmd, args) => tauri.core.invoke(cmd, args);
const $ = (id) => document.getElementById(id);

// Hosted signaling server (Fly.io). Used unless a custom or local server is chosen.
const DEFAULT_SERVER = "wss://duet-signal-shubham.fly.dev/ws";

const prefs = {
  get(k, d = "") { try { return localStorage.getItem("duet." + k) ?? d; } catch { return d; } },
  set(k, v) { try { localStorage.setItem("duet." + k, v); } catch {} },
};

/**
 * Accepts "192.168.1.20", "host:8787", "duet.example.com" or a full ws(s):// URL.
 * Bare IPs and host:port use plain ws on port 8787 (the built-in server);
 * bare domain names use wss (a hosted server behind HTTPS).
 */
function normalizeServer(s) {
  s = (s || "").trim();
  if (!s) return "";
  if (/^https?:\/\//.test(s)) s = s.replace(/^http/, "ws");
  if (!/^wss?:\/\//.test(s)) {
    const local = /^[\d.]+(:\d+)?$/.test(s) || /:\d+$/.test(s) || /^localhost\b/.test(s);
    s = (local ? "ws://" : "wss://") + s;
  }
  const u = new URL(s);
  if (!u.port && u.protocol === "ws:") u.port = "8787";
  if (u.pathname === "/" || u.pathname === "") u.pathname = "/ws";
  return u.toString();
}

function errorText(e) {
  return String(e?.message ?? e);
}
