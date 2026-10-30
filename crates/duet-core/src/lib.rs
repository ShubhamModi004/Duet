//! Duet host engine: screen capture, WebRTC transport and remote input.
//!
//! The desktop app drives this through [`host::start`]; guests are plain
//! WebRTC peers (the app's web UI), so this crate only implements hosting.

#[cfg(feature = "webrtc")]
pub mod capture;
#[cfg(feature = "webrtc")]
pub mod host;
pub mod input;
pub mod keymap;

#[cfg(feature = "webrtc")]
pub use host::{HostConfig, HostEvent, HostHandle};
