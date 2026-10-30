//! Injects a controlling guest's mouse and keyboard into the host OS.

use duet_protocol::{ControlMsg, MouseButton};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};

use crate::keymap::{is_modifier_code, key_for_code, modifier_keys};

/// Pixels per scroll "line" sent by browsers in pixel mode.
const SCROLL_PX_PER_LINE: f64 = 40.0;

pub struct Injector {
    enigo: Enigo,
    /// Display size in the coordinate space enigo uses for `Coordinate::Abs`.
    display: (i32, i32),
    /// Where we last put the cursor, to tell our moves apart from the host's.
    last_injected: Option<(i32, i32)>,
    scroll_acc: (f64, f64),
}

impl Injector {
    pub fn new() -> anyhow::Result<Self> {
        let settings = Settings { open_prompt_to_get_permissions: true, ..Default::default() };
        let enigo = Enigo::new(&settings)?;
        let display = enigo.main_display()?;
        Ok(Self { enigo, display, last_injected: None, scroll_acc: (0.0, 0.0) })
    }

    /// Converts unit coordinates to display coordinates.
    pub fn to_display(&self, x: f64, y: f64) -> (i32, i32) {
        to_display(self.display, x, y)
    }

    /// The host's real cursor, normalized; `None` if the platform won't say.
    pub fn cursor_normalized(&self) -> Option<(f64, f64)> {
        let (x, y) = self.enigo.location().ok()?;
        let (w, h) = self.display;
        (w > 0 && h > 0).then(|| (x as f64 / w as f64, y as f64 / h as f64))
    }

    /// True when the cursor sits somewhere we didn't put it: the host grabbed
    /// their own mouse, which takes control back (Tuple-style).
    pub fn host_moved_mouse(&self) -> bool {
        let (Some(expected), Ok(actual)) = (self.last_injected, self.enigo.location()) else {
            return false;
        };
        (expected.0 - actual.0).abs() > 3 || (expected.1 - actual.1).abs() > 3
    }

    pub fn forget_position(&mut self) {
        self.last_injected = None;
    }

    pub fn apply(&mut self, msg: &ControlMsg) -> anyhow::Result<()> {
        match msg.clone().clamped() {
            ControlMsg::MouseMove { x, y } => self.move_to(x, y)?,
            ControlMsg::MouseDown { x, y, button } => {
                self.move_to(x, y)?;
                self.enigo.button(button_of(button), Direction::Press)?;
            }
            ControlMsg::MouseUp { x, y, button } => {
                self.move_to(x, y)?;
                self.enigo.button(button_of(button), Direction::Release)?;
            }
            ControlMsg::Scroll { dx, dy } => {
                self.scroll_acc.0 += dx / SCROLL_PX_PER_LINE;
                self.scroll_acc.1 += dy / SCROLL_PX_PER_LINE;
                let (lx, ly) = (self.scroll_acc.0.trunc(), self.scroll_acc.1.trunc());
                self.scroll_acc.0 -= lx;
                self.scroll_acc.1 -= ly;
                if ly != 0.0 {
                    self.enigo.scroll(ly as i32, Axis::Vertical)?;
                }
                if lx != 0.0 {
                    self.enigo.scroll(lx as i32, Axis::Horizontal)?;
                }
            }
            ControlMsg::KeyDown { code, modifiers } => {
                if is_modifier_code(&code) {
                    return Ok(());
                }
                let Some(key) = key_for_code(&code) else { return Ok(()) };
                let mods = modifier_keys(&modifiers);
                for m in &mods {
                    self.enigo.key(*m, Direction::Press)?;
                }
                let r = self.enigo.key(key, Direction::Click);
                for m in mods.iter().rev() {
                    self.enigo.key(*m, Direction::Release)?;
                }
                r?;
            }
            // Keys are sent as clicks on key_down, so key_up has nothing to do.
            ControlMsg::KeyUp { .. } => {}
            ControlMsg::Text { text } => self.enigo.text(&text)?,
            _ => {}
        }
        Ok(())
    }

    fn move_to(&mut self, x: f64, y: f64) -> anyhow::Result<()> {
        let p = self.to_display(x, y);
        self.enigo.move_mouse(p.0, p.1, Coordinate::Abs)?;
        self.last_injected = Some(p);
        Ok(())
    }
}

fn to_display((w, h): (i32, i32), x: f64, y: f64) -> (i32, i32) {
    let px = (x * w as f64).round() as i32;
    let py = (y * h as f64).round() as i32;
    (px.clamp(0, (w - 1).max(0)), py.clamp(0, (h - 1).max(0)))
}

fn button_of(b: MouseButton) -> Button {
    match b {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    }
}

#[cfg(test)]
mod tests {
    use super::to_display;

    #[test]
    fn maps_unit_square_onto_display() {
        assert_eq!(to_display((1920, 1080), 0.0, 0.0), (0, 0));
        assert_eq!(to_display((1920, 1080), 0.5, 0.5), (960, 540));
        assert_eq!(to_display((1920, 1080), 1.0, 1.0), (1919, 1079));
    }
}
