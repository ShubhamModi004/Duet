# Duet architecture

## Goals

1. Latency low enough that driving someone else's computer feels local.
2. Text stays crisp: favour resolution over frame rate when bandwidth drops.
3. One codebase for macOS and Windows.

## Why Rust + Tauri

| Need | Choice | Why |
| --- | --- | --- |
| Screen capture, encoding, transport | Rust + LiveKit's `libwebrtc` bindings | Google's WebRTC engine: hardware encoders (VideoToolbox, Media Foundation), congestion control, echo cancellation and NAT traversal, all battle-tested. Its desktop capturer already wraps ScreenCaptureKit and DXGI. |
| Remote input | `enigo` | One API over CGEvent (macOS) and SendInput (Windows). |
| UI | Tauri 2 | Native window with a system webview: ~10 MB installers instead of Electron's ~150 MB, and the guest viewer gets the webview's built-in WebRTC and hardware video decoding for free. |
| Signaling | axum WebSocket server | Tiny, stateless apart from in-memory rooms; embeddable in the app. |

The performance-sensitive path on the host (capture → convert → encode → send,
and receive → inject) is native code. JavaScript only renders the guest view
and forwards input events, which the webview already does natively.

## Sessions

- One **host** per room shares their primary display. Up to three **guests**.
- The host is the WebRTC offerer to each guest. Each peer connection carries:
  - the screen video track (shared source, one encoder per guest),
  - the host's microphone, and the guest's microphone in return,
  - a reliable, ordered data channel labelled `control`.
- Guests do not connect to each other. Pointer messages are relayed by the host.

## Control hand-off

`duet_protocol::ControlState` is the single source of truth, owned by the host:

- A guest sends `request_control`. The host is prompted, unless auto-grant is
  on.
- At most one guest controls at a time; granting another replaces them.
- Input from anyone else is dropped before it reaches the OS.
- The host takes control back by clicking **Take back control** or simply by
  moving their own mouse (the input thread notices the cursor is somewhere it
  did not put it).
- A controller who leaves loses control.

## Coordinates and keys

- Mouse positions are normalized to `0..1` of the shared display, so guests
  never need the host's resolution or scale factor.
- Printable characters are sent as `text` (works across keyboard layouts).
  Shortcuts and special keys are sent as W3C `KeyboardEvent.code` plus
  modifiers.
- `modifiers.command` means "the platform's shortcut key": ⌘ on a Mac host,
  Ctrl on Windows. A Mac guest's ⌘C and a Windows guest's Ctrl+C both copy on
  either host.

## Known limits (v0.1)

- Primary display only.
- STUN only, no TURN yet, so some corporate networks will fail to connect.
- Unsigned builds.
