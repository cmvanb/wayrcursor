//! Debug tool: records raw evdev events from pointing devices to a trace file.
//!
//! Usage: `record OUT_FILE SECONDS DEVICE...`. Recording starts at the first motion
//! and lasts SECONDS. Each line is `device_index seconds type code value`.

use std::io::Write;
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use evdev::Device;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(out), Some(seconds)) = (args.next(), args.next()) else {
        anyhow::bail!("usage: record OUT_FILE SECONDS DEVICE...");
    };
    let length = Duration::from_secs_f64(seconds.parse()?);
    let (tx, rx) = mpsc::channel();
    for (index, path) in args.enumerate() {
        let mut device = Device::open(&path)?;
        eprintln!("{index}: {path} {}", device.name().unwrap_or("?"));
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(events) = device.fetch_events() {
                for e in events {
                    let t = e.timestamp().duration_since(UNIX_EPOCH).unwrap_or_default();
                    if tx
                        .send((
                            index,
                            t.as_secs_f64(),
                            e.event_type().0,
                            e.code(),
                            e.value(),
                        ))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        });
    }
    drop(tx);
    let mut file = std::io::BufWriter::new(std::fs::File::create(&out)?);
    let first = rx.recv()?;
    eprintln!("recording");
    let deadline = Instant::now() + length;
    let mut write = |(i, t, ty, code, v): (usize, f64, u16, u16, i32)| {
        writeln!(file, "{i} {:.6} {ty} {code} {v}", t - first.1)
    };
    write(first)?;
    while let Ok(event) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        write(event)?;
    }
    eprintln!("done");
    Ok(())
}
