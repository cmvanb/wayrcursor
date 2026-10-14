use std::time::Instant;

use evdev::{
    AbsoluteAxisCode, EventSummary, InputEvent, KeyCode, RelativeAxisCode, SynchronizationCode,
};

use crate::shake::Motion;

/// Mice report counts, not distance; 1000 CPI is a common sensor default.
const MM_PER_COUNT: f32 = 25.4 / 1000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeviceKind {
    Relative,
    /// Units per millimetre on each axis.
    Touchpad {
        x_res: f32,
        y_res: f32,
    },
}

/// Turns one device's raw event stream into per-report motion in millimetres.
pub struct Translator {
    kind: DeviceKind,
    pending: (f32, f32),
    position: (Option<i32>, Option<i32>),
    last: Option<(i32, i32)>,
    one_finger: bool,
}

impl Translator {
    pub fn new(kind: DeviceKind) -> Self {
        Self {
            kind,
            pending: (0.0, 0.0),
            position: (None, None),
            last: None,
            one_finger: false,
        }
    }

    pub fn translate(&mut self, event: InputEvent, at: Instant) -> Option<Motion> {
        match event.destructure() {
            EventSummary::RelativeAxis(_, RelativeAxisCode::REL_X, v) => {
                self.pending.0 += v as f32 * MM_PER_COUNT
            }
            EventSummary::RelativeAxis(_, RelativeAxisCode::REL_Y, v) => {
                self.pending.1 += v as f32 * MM_PER_COUNT
            }
            EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_X, v) => self.position.0 = Some(v),
            EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_Y, v) => self.position.1 = Some(v),
            EventSummary::Key(_, KeyCode::BTN_TOOL_FINGER, v) => {
                self.one_finger = v != 0;
                self.last = None;
            }
            EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
                return self.report(at);
            }
            _ => {}
        }
        None
    }

    fn report(&mut self, at: Instant) -> Option<Motion> {
        let (dx, dy) = match self.kind {
            DeviceKind::Relative => std::mem::take(&mut self.pending),
            DeviceKind::Touchpad { x_res, y_res } => {
                let current = match self.position {
                    (Some(x), Some(y)) if self.one_finger => (x, y),
                    _ => return None,
                };
                let last = self.last.replace(current)?;
                (
                    (current.0 - last.0) as f32 / x_res,
                    (current.1 - last.1) as f32 / y_res,
                )
            }
        };
        (dx != 0.0 || dy != 0.0).then_some(Motion { dx, dy, at })
    }
}

#[cfg(test)]
mod tests {
    use evdev::EventType;

    use super::*;

    fn rel(code: RelativeAxisCode, v: i32) -> InputEvent {
        InputEvent::new(EventType::RELATIVE.0, code.0, v)
    }
    fn abs(code: AbsoluteAxisCode, v: i32) -> InputEvent {
        InputEvent::new(EventType::ABSOLUTE.0, code.0, v)
    }
    fn key(code: KeyCode, v: i32) -> InputEvent {
        InputEvent::new(EventType::KEY.0, code.0, v)
    }
    fn syn() -> InputEvent {
        InputEvent::new(
            EventType::SYNCHRONIZATION.0,
            SynchronizationCode::SYN_REPORT.0,
            0,
        )
    }

    fn run(t: &mut Translator, events: &[InputEvent]) -> Vec<(f32, f32)> {
        let at = Instant::now();
        events
            .iter()
            .filter_map(|e| t.translate(*e, at))
            .map(|m| (m.dx, m.dy))
            .collect()
    }

    fn touchpad() -> Translator {
        Translator::new(DeviceKind::Touchpad {
            x_res: 10.0,
            y_res: 20.0,
        })
    }

    #[test]
    fn mouse_counts_become_millimetres_per_report() {
        let mut t = Translator::new(DeviceKind::Relative);
        let out = run(
            &mut t,
            &[
                rel(RelativeAxisCode::REL_X, 600),
                rel(RelativeAxisCode::REL_Y, -400),
                syn(),
                rel(RelativeAxisCode::REL_WHEEL, 1),
                syn(),
            ],
        );
        assert_eq!(out.len(), 1);
        assert!(
            (out[0].0 - 15.24).abs() < 1e-3 && (out[0].1 + 10.16).abs() < 1e-3,
            "{out:?}"
        );
    }

    #[test]
    fn one_finger_touchpad_motion_becomes_millimetres() {
        let mut t = touchpad();
        let out = run(
            &mut t,
            &[
                key(KeyCode::BTN_TOOL_FINGER, 1),
                abs(AbsoluteAxisCode::ABS_X, 100),
                abs(AbsoluteAxisCode::ABS_Y, 100),
                syn(),
                abs(AbsoluteAxisCode::ABS_X, 150),
                abs(AbsoluteAxisCode::ABS_Y, 60),
                syn(),
            ],
        );
        assert_eq!(out, vec![(5.0, -2.0)]);
    }

    #[test]
    fn lifting_the_finger_does_not_jump() {
        let mut t = touchpad();
        let out = run(
            &mut t,
            &[
                key(KeyCode::BTN_TOOL_FINGER, 1),
                abs(AbsoluteAxisCode::ABS_X, 100),
                abs(AbsoluteAxisCode::ABS_Y, 100),
                syn(),
                key(KeyCode::BTN_TOOL_FINGER, 0),
                syn(),
                key(KeyCode::BTN_TOOL_FINGER, 1),
                abs(AbsoluteAxisCode::ABS_X, 900),
                abs(AbsoluteAxisCode::ABS_Y, 900),
                syn(),
                abs(AbsoluteAxisCode::ABS_X, 910),
                syn(),
            ],
        );
        assert_eq!(out, vec![(1.0, 0.0)]);
    }

    #[test]
    fn two_finger_scrolling_is_ignored() {
        let mut t = touchpad();
        let out = run(
            &mut t,
            &[
                key(KeyCode::BTN_TOOL_FINGER, 0),
                key(KeyCode::BTN_TOOL_DOUBLETAP, 1),
                abs(AbsoluteAxisCode::ABS_X, 100),
                abs(AbsoluteAxisCode::ABS_Y, 100),
                syn(),
                abs(AbsoluteAxisCode::ABS_X, 300),
                syn(),
            ],
        );
        assert_eq!(out, vec![]);
    }
}
