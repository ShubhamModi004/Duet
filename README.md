# Duet

Fast remote pair programming for **macOS and Windows**. Share your screen, hand
over mouse and keyboard, and talk in real time over peer-to-peer WebRTC.

- **Screen sharing**: native capture (ScreenCaptureKit on macOS, DXGI on
  Windows), hardware video encoding, tuned to keep text sharp.
- **Remote control**: your pair drives your mouse and keyboard when you allow
  it. Move your own mouse and control snaps back to you.
- **Voice**: built-in audio with echo cancellation.
- **Pointers**: guests can point at things on your screen without taking
  control.
- **Peer to peer**: video and input go directly between computers. The small
  signaling server only introduces peers and never sees your screen.

## How it fits together

```
┌──────────── Host (Mac or PC) ────────────┐          ┌───── Guest (Mac or PC) ─────┐
│ Tauri app                                │          │ Tauri app (web UI)          │
│  ├─ duet-core (Rust)                     │  WebRTC  │  ├─ <video> shared screen   │
│  │   ├─ screen capture → H.264/VP8 ──────┼─────────►│  ├─ mic ↔ speaker           │
│  │   ├─ mic / speaker  ◄─────────────────┼─────────►│  └─ control data channel    │
│  │   └─ input injection ◄── "control" ───┼──────────┤     (mouse, keys, pointer)  │
│  └─ built-in signaling server (optional) │          │                             │
└──────────────────┬───────────────────────┘          └──────────────┬──────────────┘
                   └──────── duet-signal (WebSocket: rooms + SDP/ICE) ┘
```

| Path | What it is |
| --- | --- |
| `crates/duet-protocol` | Wire format shared by everything, plus the control hand-off rules. |
| `crates/duet-signal` | Signaling server (axum). Embedded in the app, or run on its own. |
| `crates/duet-core` | Host engine: capture, WebRTC (LiveKit's libwebrtc), voice, input (enigo). |
| `app/src-tauri` | Desktop app shell (Tauri 2). |
| `app/ui` | Host panel, guest viewer and pointer overlay (plain HTML/JS). |

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and
[docs/PROTOCOL.md](docs/PROTOCOL.md) for details.

## Download

Every push to `main` builds installers in GitHub Actions. Open the latest
**CI** run and download `Duet-macos-arm64` (`.dmg`) or `Duet-windows-x64`
(`.msi` / `.exe`). Builds are unsigned for now: on macOS right-click the app
and choose **Open** the first time.

## Run from source

**macOS** (Apple silicon or Intel, macOS 13+):

```bash
./scripts/dev-mac.sh          # run in dev mode
./scripts/dev-mac.sh build    # build Duet.dmg
```

**Windows 10/11**: install [Rust](https://rustup.rs), Node.js and Visual
Studio Build Tools ("Desktop development with C++"), then:

```powershell
.\scripts\dev-windows.ps1          # run in dev mode
.\scripts\dev-windows.ps1 build    # build the .msi installer
```

The first build downloads a prebuilt libwebrtc (~100 MB) and takes a few
minutes.

## Pairing

1. **Host** clicks **Start sharing** and gets a room code such as
   `kip-moza-tule` plus the address guests should use as **Server**.
2. **Guest** enters the code and server and clicks **Join**.
3. The guest picks **Point** to show things, or **Control** to ask for the
   keyboard and mouse. The host clicks **Allow**, or ticks "Let guests take
   control without asking".

### Permissions (macOS host)

The first time you share, macOS asks for:

- **Screen Recording** to share your screen,
- **Accessibility** so your pair can control the mouse and keyboard,
- **Microphone** for voice.

Grant them in System Settings → Privacy & Security, then restart Duet.

### Pairing over the internet

The built-in server works on the same network. To pair across networks, run
the signaling server somewhere both of you can reach and put its address in
**Server** on both sides:

```bash
docker build -f Dockerfile.signal -t duet-signal .
docker run -p 8787:8787 duet-signal
```

Peers connect directly using STUN. Behind strict corporate NATs you will also
need a TURN server (for example coturn); TURN support is on the roadmap.

## Development

```bash
cargo test -p duet-protocol -p duet-signal        # protocol + server
cargo test -p duet-core --no-default-features     # keymap + input
```

On Linux, building the WebRTC engine needs clang 21+, so CI only builds the
desktop app on macOS and Windows.

## Roadmap

- TURN server support and a hosted signaling server
- Multiple displays and window-only sharing
- Code signing and notarization for macOS and Windows
- Drawing/annotations on the shared screen
- Browser guests (join from any browser via the signaling server)

## License

MIT
