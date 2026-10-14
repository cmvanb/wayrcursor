use std::cell::RefCell;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use evdev::{
    AbsoluteAxisCode, Device, EventSummary, InputEvent, KeyCode, PropType, RelativeAxisCode,
    SynchronizationCode,
};
use inotify::{Inotify, WatchMask};
use smithay_client_toolkit::reexports::calloop::{
    Interest, LoopHandle, Mode, PostAction, generic::Generic,
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

/// Watches every pointing device, including hotplugged ones, and reports their motion.
pub fn watch<D: 'static>(
    handle: &LoopHandle<'static, D>,
    on_motion: impl FnMut(Motion, &mut D) + 'static,
) -> anyhow::Result<()> {
    let watcher = Rc::new(Watcher {
        handle: handle.clone(),
        on_motion: RefCell::new(Box::new(on_motion)),
        open: RefCell::default(),
    });
    let inotify = Inotify::init()?;
    inotify
        .watches()
        .add(INPUT_DIR, WatchMask::CREATE | WatchMask::ATTRIB)?;
    let hotplug = watcher.clone();
    handle
        .insert_source(
            Generic::new(inotify, Interest::READ, Mode::Level),
            move |_, inotify, _| {
                let mut buffer = [0; 4096];
                let names: Vec<_> = match unsafe { inotify.get_mut() }.read_events(&mut buffer) {
                    Ok(events) => events
                        .filter_map(|e| e.name.map(|n| n.to_owned()))
                        .collect(),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => Vec::new(),
                    Err(e) => return Err(e),
                };
                for name in names {
                    hotplug.open(Path::new(INPUT_DIR).join(name));
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    for entry in std::fs::read_dir(INPUT_DIR)? {
        watcher.open(entry?.path());
    }
    Ok(())
}

const INPUT_DIR: &str = "/dev/input";

type MotionHandler<D> = Box<dyn FnMut(Motion, &mut D)>;

struct Watcher<D: 'static> {
    handle: LoopHandle<'static, D>,
    on_motion: RefCell<MotionHandler<D>>,
    open: RefCell<HashSet<PathBuf>>,
}

impl<D: 'static> Watcher<D> {
    fn open(self: &Rc<Self>, path: PathBuf) {
        let is_event_node = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("event"));
        if !is_event_node || self.open.borrow().contains(&path) {
            return;
        }
        let Ok(device) = Device::open(&path) else {
            return;
        };
        let Some(kind) = classify(&device) else {
            return;
        };
        if device.set_nonblocking(true).is_err() {
            return;
        }
        let mut translator = Translator::new(kind);
        let watcher = self.clone();
        let source_path = path.clone();
        let inserted = self.handle.insert_source(
            Generic::new(device, Interest::READ, Mode::Level),
            move |_, device, state| {
                let now = Instant::now();
                let events: Vec<InputEvent> = match unsafe { device.get_mut() }.fetch_events() {
                    Ok(events) => events.collect(),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        return Ok(PostAction::Continue);
                    }
                    Err(_) => {
                        watcher.open.borrow_mut().remove(&source_path);
                        return Ok(PostAction::Remove);
                    }
                };
                let mut on_motion = watcher.on_motion.borrow_mut();
                for motion in events
                    .into_iter()
                    .filter_map(|e| translator.translate(e, now))
                {
                    on_motion(motion, state);
                }
                Ok(PostAction::Continue)
            },
        );
        if inserted.is_ok() {
            self.open.borrow_mut().insert(path);
        }
    }
}

fn classify(device: &Device) -> Option<DeviceKind> {
    let rel = device.supported_relative_axes();
    if rel
        .is_some_and(|r| r.contains(RelativeAxisCode::REL_X) && r.contains(RelativeAxisCode::REL_Y))
    {
        return Some(DeviceKind::Relative);
    }
    let is_touchpad = device.properties().contains(PropType::POINTER)
        && device
            .supported_keys()
            .is_some_and(|k| k.contains(KeyCode::BTN_TOOL_FINGER))
        && device.supported_absolute_axes().is_some_and(|a| {
            a.contains(AbsoluteAxisCode::ABS_X) && a.contains(AbsoluteAxisCode::ABS_Y)
        });
    if !is_touchpad {
        return None;
    }
    let state = device.get_abs_state().ok()?;
    // Without a reported resolution, assume the pad is about 100mm across.
    let resolution = |code: AbsoluteAxisCode| {
        let info = state[code.0 as usize];
        if info.resolution > 0 {
            info.resolution as f32
        } else {
            ((info.maximum - info.minimum) as f32 / 100.0).max(1.0)
        }
    };
    Some(DeviceKind::Touchpad {
        x_res: resolution(AbsoluteAxisCode::ABS_X),
        y_res: resolution(AbsoluteAxisCode::ABS_Y),
    })
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
