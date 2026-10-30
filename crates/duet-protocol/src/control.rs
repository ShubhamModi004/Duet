//! Who may drive the host's mouse and keyboard.
//!
//! The host owns this state. Exactly zero or one guest holds control at a
//! time; input from anyone else is dropped. The Swift host mirrors this logic
//! in `ControlGate.swift`; the tests here are the reference behaviour.

use crate::{ControlMsg, PeerId};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ControlState {
    controller: Option<PeerId>,
    pending: Vec<PeerId>,
    /// When true, requests are granted immediately (Tuple-style "trust mode").
    pub auto_grant: bool,
}

/// What the host UI should do after a state change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlEvent {
    /// Ask the host "<peer> wants control" (pending requests).
    Prompt(PeerId),
    Granted(PeerId),
    Revoked(PeerId),
}

impl ControlState {
    pub fn controller(&self) -> Option<&PeerId> {
        self.controller.as_ref()
    }

    pub fn pending(&self) -> &[PeerId] {
        &self.pending
    }

    /// Handle a hand-off message received from `from` over the data channel.
    pub fn on_peer_msg(&mut self, from: &PeerId, msg: &ControlMsg) -> Option<ControlEvent> {
        match msg {
            ControlMsg::RequestControl => {
                if self.controller.as_ref() == Some(from) {
                    return None;
                }
                if self.auto_grant {
                    return Some(self.grant(from.clone()));
                }
                if !self.pending.contains(from) {
                    self.pending.push(from.clone());
                }
                Some(ControlEvent::Prompt(from.clone()))
            }
            ControlMsg::ReleaseControl if self.controller.as_ref() == Some(from) => {
                self.controller = None;
                Some(ControlEvent::Revoked(from.clone()))
            }
            _ => None,
        }
    }

    /// Host approves a guest. Replaces any current controller.
    pub fn grant(&mut self, to: PeerId) -> ControlEvent {
        self.pending.retain(|p| p != &to);
        self.controller = Some(to.clone());
        ControlEvent::Granted(to)
    }

    /// Host takes control back (also called when the host touches their own
    /// mouse/keyboard).
    pub fn revoke(&mut self) -> Option<ControlEvent> {
        self.controller.take().map(ControlEvent::Revoked)
    }

    pub fn deny(&mut self, peer: &PeerId) {
        self.pending.retain(|p| p != peer);
    }

    pub fn on_peer_left(&mut self, peer: &PeerId) -> Option<ControlEvent> {
        self.deny(peer);
        if self.controller.as_ref() == Some(peer) {
            return self.revoke();
        }
        None
    }

    /// Should the host inject this message from `from`?
    pub fn accepts_input(&self, from: &PeerId, msg: &ControlMsg) -> bool {
        msg.is_input() && self.controller.as_ref() == Some(from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PeerId {
        s.to_string()
    }

    #[test]
    fn request_prompts_then_grant_allows_input() {
        let mut s = ControlState::default();
        let mv = ControlMsg::MouseMove { x: 0.5, y: 0.5 };
        assert!(!s.accepts_input(&p("a"), &mv));
        assert_eq!(s.on_peer_msg(&p("a"), &ControlMsg::RequestControl), Some(ControlEvent::Prompt(p("a"))));
        assert_eq!(s.pending(), &[p("a")]);
        assert_eq!(s.grant(p("a")), ControlEvent::Granted(p("a")));
        assert!(s.pending().is_empty());
        assert!(s.accepts_input(&p("a"), &mv));
        assert!(!s.accepts_input(&p("b"), &mv));
    }

    #[test]
    fn non_input_is_never_injected() {
        let mut s = ControlState::default();
        s.grant(p("a"));
        let ptr = ControlMsg::Pointer { x: 0.1, y: 0.1, color: "#f00".into() };
        assert!(!s.accepts_input(&p("a"), &ptr));
    }

    #[test]
    fn auto_grant_skips_prompt() {
        let mut s = ControlState { auto_grant: true, ..Default::default() };
        assert_eq!(s.on_peer_msg(&p("a"), &ControlMsg::RequestControl), Some(ControlEvent::Granted(p("a"))));
        assert_eq!(s.controller(), Some(&p("a")));
    }

    #[test]
    fn grant_replaces_previous_controller() {
        let mut s = ControlState::default();
        s.grant(p("a"));
        s.grant(p("b"));
        assert_eq!(s.controller(), Some(&p("b")));
    }

    #[test]
    fn release_and_revoke() {
        let mut s = ControlState::default();
        s.grant(p("a"));
        assert_eq!(s.on_peer_msg(&p("b"), &ControlMsg::ReleaseControl), None);
        assert_eq!(s.on_peer_msg(&p("a"), &ControlMsg::ReleaseControl), Some(ControlEvent::Revoked(p("a"))));
        assert_eq!(s.revoke(), None);
        s.grant(p("a"));
        assert_eq!(s.revoke(), Some(ControlEvent::Revoked(p("a"))));
    }

    #[test]
    fn leaving_peer_loses_control() {
        let mut s = ControlState::default();
        s.on_peer_msg(&p("b"), &ControlMsg::RequestControl);
        s.grant(p("a"));
        assert_eq!(s.on_peer_left(&p("a")), Some(ControlEvent::Revoked(p("a"))));
        assert_eq!(s.on_peer_left(&p("b")), None);
        assert!(s.pending().is_empty());
    }
}
