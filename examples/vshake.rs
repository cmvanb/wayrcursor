//! Test driver: a button-less uinput motion device that shakes on demand.
//!
//! Without buttons udev does not tag it as a mouse, so compositors leave it alone
//! while `wayrcursor daemon` still reads it. Each stdin line `shake N` emits N brisk
//! horizontal strokes; `drift N` emits N slow strokes that must not trigger.

use std::io::BufRead;
use std::time::Duration;

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, RelativeAxisCode};

fn main() -> anyhow::Result<()> {
    let axes = AttributeSet::from_iter([RelativeAxisCode::REL_X, RelativeAxisCode::REL_Y]);
    let mut device = VirtualDevice::builder()?
        .name("wayrcursor-vshake")
        .with_relative_axes(&axes)?
        .build()?;
    for node in device.enumerate_dev_nodes_blocking()? {
        println!("{}", node?.display());
    }
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let (pace, count) = match line.split_whitespace().collect::<Vec<_>>().as_slice() {
            ["shake", n] => (Duration::from_millis(8), n.parse::<usize>()?),
            ["drift", n] => (Duration::from_millis(60), n.parse::<usize>()?),
            _ => {
                eprintln!("vshake: unknown command {line:?}");
                continue;
            }
        };
        for stroke in 0..count {
            let sign = if stroke % 2 == 0 { 1 } else { -1 };
            for _ in 0..10 {
                device.emit(&[InputEvent::new(
                    EventType::RELATIVE.0,
                    RelativeAxisCode::REL_X.0,
                    sign * 100,
                )])?;
                std::thread::sleep(pace);
            }
        }
        println!("done");
    }
    Ok(())
}
