//! Test driver: keeps a wlr virtual pointer alive and moves it from stdin commands.
//!
//! Usage: `vpointer <layout-width> <layout-height>`, then lines of
//! `abs X Y` (layout coordinates) or `rel DX DY`.

use std::io::BufRead;
use std::time::Instant;

use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

#[derive(Default)]
struct Globals {
    seat: Option<wl_seat::WlSeat>,
    manager: Option<ZwlrVirtualPointerManagerV1>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Globals {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, .. } = event {
            match interface.as_str() {
                "wl_seat" => state.seat = Some(registry.bind(name, 1, qh, ())),
                "zwlr_virtual_pointer_manager_v1" => {
                    state.manager = Some(registry.bind(name, 1, qh, ()))
                }
                _ => {}
            }
        }
    }
}

delegate_noop!(Globals: ignore wl_seat::WlSeat);
delegate_noop!(Globals: ZwlrVirtualPointerManagerV1);
delegate_noop!(Globals: ZwlrVirtualPointerV1);

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1).map(|a| a.parse::<u32>());
    let (width, height) = match (args.next(), args.next()) {
        (Some(Ok(w)), Some(Ok(h))) => (w, h),
        _ => anyhow::bail!("usage: vpointer <layout-width> <layout-height>"),
    };

    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut globals = Globals::default();
    queue.roundtrip(&mut globals)?;
    let manager = globals.manager.as_ref().ok_or_else(|| anyhow::anyhow!("no virtual pointer manager"))?;
    let pointer = manager.create_virtual_pointer(globals.seat.as_ref(), &qh, ());
    queue.roundtrip(&mut globals)?;

    let start = Instant::now();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let words: Vec<&str> = line.split_whitespace().collect();
        let time = start.elapsed().as_millis() as u32;
        match words.as_slice() {
            ["abs", x, y] => pointer.motion_absolute(time, x.parse()?, y.parse()?, width, height),
            ["rel", dx, dy] => pointer.motion(time, dx.parse()?, dy.parse()?),
            _ => {
                eprintln!("vpointer: unknown command {line:?}");
                continue;
            }
        }
        pointer.frame();
        queue.roundtrip(&mut globals)?;
    }
    Ok(())
}
