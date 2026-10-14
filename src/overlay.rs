use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{
            PointerEvent, PointerEventKind, PointerHandler, cursor_shape::CursorShapeManager,
        },
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
};
use wayland_client::{
    Connection, Proxy, QueueHandle,
    globals::GlobalList,
    protocol::{
        wl_output::WlOutput, wl_pointer::WlPointer, wl_seat::WlSeat, wl_surface::WlSurface,
    },
};
use wayland_protocols::wp::{
    cursor_shape::v1::client::{
        wp_cursor_shape_device_v1::Shape, wp_cursor_shape_device_v1::WpCursorShapeDeviceV1,
    },
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

use crate::anim::{Animation, Timing};
use crate::config::{Config, RingStyle};
use crate::gpu::{Gpu, Target};
use crate::ring;

/// How long to wait for the compositor to reveal the cursor before giving up and releasing input.
const SEEK_TIMEOUT: Duration = Duration::from_millis(500);

enum RingState {
    Idle,
    Seeking { deadline: Instant },
    Shown(Animation),
}

struct Cursor {
    surface: WlSurface,
    position: (f32, f32),
}

/// One fullscreen layer surface per output. Only the one under the cursor draws the ring.
struct Surface {
    output: WlOutput,
    layer: LayerSurface,
    viewport: WpViewport,
    fractional: Option<WpFractionalScaleV1>,
    scale120: u32,
    logical: Option<(u32, u32)>,
    target: Option<Target>,
    painted: bool,
    frame_pending: bool,
}

impl Surface {
    fn buffer_size(&self) -> Option<(i32, i32)> {
        let (w, h) = self.logical?;
        let px = |v: u32| ((v * self.scale120 + 60) / 120).max(1) as i32;
        Some((px(w), px(h)))
    }

    fn scale(&self) -> f32 {
        self.scale120 as f32 / 120.0
    }
}

pub struct Overlay {
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    viewporter: WpViewporter,
    fractional: Option<WpFractionalScaleManagerV1>,
    cursor_shape: Option<CursorShapeManager>,
    virtual_pointer: Option<ZwlrVirtualPointerManagerV1>,
    pointers: Vec<(WlPointer, Option<WpCursorShapeDeviceV1>)>,
    gpu: Gpu,
    surfaces: Vec<Surface>,
    cursor: Option<Cursor>,
    ring: RingState,
    style: RingStyle,
    timing: Timing,
    epoch: Instant,
}

impl Overlay {
    pub fn new(
        conn: &Connection,
        globals: &GlobalList,
        qh: &QueueHandle<Self>,
    ) -> anyhow::Result<Self> {
        let gpu = Gpu::new(conn.backend().display_ptr().cast())?;
        let config = Config::default();
        Ok(Self {
            registry: RegistryState::new(globals),
            seats: SeatState::new(globals, qh),
            outputs: OutputState::new(globals, qh),
            compositor: CompositorState::bind(globals, qh)?,
            layer_shell: LayerShell::bind(globals, qh)?,
            viewporter: globals.bind(qh, 1..=1, Ignore)?,
            fractional: globals.bind(qh, 1..=1, Ignore).ok(),
            cursor_shape: CursorShapeManager::bind(globals, qh).ok(),
            virtual_pointer: globals.bind(qh, 1..=2, Ignore).ok(),
            pointers: Vec::new(),
            gpu,
            surfaces: Vec::new(),
            cursor: None,
            ring: RingState::Idle,
            style: config.ring,
            timing: config.timing(),
            epoch: Instant::now(),
        })
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.ring, RingState::Idle)
    }

    /// Shows the ring, or keeps an already visible ring on screen.
    pub fn trigger(&mut self, qh: &QueueHandle<Self>, config: &Config) {
        self.style = config.ring;
        self.timing = config.timing();
        let now = Instant::now();
        match &mut self.ring {
            RingState::Idle => {
                self.ring = RingState::Seeking {
                    deadline: now + SEEK_TIMEOUT,
                };
                let outputs: Vec<WlOutput> = self.outputs.outputs().collect();
                for output in outputs {
                    self.add_surface(qh, output);
                }
            }
            RingState::Seeking { .. } => {}
            RingState::Shown(animation) => animation.retrigger(now),
        }
    }

    /// When the event loop must wake without Wayland traffic.
    pub fn wake_at(&self) -> Option<Instant> {
        match &self.ring {
            RingState::Seeking { deadline } => Some(*deadline),
            RingState::Shown(_) if self.cursor.is_none() => {
                Some(Instant::now() + Duration::from_millis(50))
            }
            _ => None,
        }
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        let expired = match &mut self.ring {
            RingState::Seeking { deadline } => now >= *deadline,
            RingState::Shown(animation) => self.cursor.is_none() && animation.pose(now).is_none(),
            RingState::Idle => false,
        };
        if expired {
            self.teardown();
        }
    }

    fn add_surface(&mut self, qh: &QueueHandle<Self>, output: WlOutput) {
        let wl_surface = self.compositor.create_surface(qh);
        let viewport = self.viewporter.get_viewport(&wl_surface, qh, Ignore);
        let fractional = self
            .fractional
            .as_ref()
            .map(|m| m.get_fractional_scale(&wl_surface, qh, ScaleOf(wl_surface.clone())));
        let scale120 = self
            .outputs
            .info(&output)
            .map_or(1, |i| i.scale_factor.max(1)) as u32
            * 120;
        let layer = self.layer_shell.create_layer_surface(
            qh,
            wl_surface,
            Layer::Overlay,
            Some("wayrcursor"),
            Some(&output),
        );
        layer.set_anchor(Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.commit();
        self.surfaces.push(Surface {
            output,
            layer,
            viewport,
            fractional,
            scale120,
            logical: None,
            target: None,
            painted: false,
            frame_pending: false,
        });
    }

    fn teardown(&mut self) {
        for surface in self.surfaces.drain(..) {
            if let Some(target) = surface.target {
                self.gpu.destroy_target(target);
            }
            surface.viewport.destroy();
            if let Some(f) = surface.fractional {
                f.destroy();
            }
        }
        self.cursor = None;
        self.ring = RingState::Idle;
    }

    /// Provokes `wl_pointer.enter` on a freshly mapped overlay; compositors skip it for a still cursor.
    fn nudge_pointer(&self, qh: &QueueHandle<Self>) {
        let Some(manager) = &self.virtual_pointer else {
            return;
        };
        let pointer = manager.create_virtual_pointer(None, qh, Ignore);
        pointer.motion(self.epoch.elapsed().as_millis() as u32, 0.0, 0.0);
        pointer.frame();
        pointer.destroy();
    }

    fn index_of(&self, wl_surface: &WlSurface) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|s| s.layer.wl_surface() == wl_surface)
    }

    fn render(&mut self, qh: &QueueHandle<Self>, index: usize) {
        let now = Instant::now();
        let is_cursor_surface = self
            .cursor
            .as_ref()
            .is_some_and(|c| &c.surface == self.surfaces[index].layer.wl_surface());
        let pose = match &mut self.ring {
            RingState::Shown(animation) if is_cursor_surface => match animation.pose(now) {
                Some(pose) => Some(pose),
                None => return self.teardown(),
            },
            _ => None,
        };
        let surface = &mut self.surfaces[index];
        let scale = surface.scale();
        let (Some(logical), Some(size), Some(target)) = (
            surface.logical,
            surface.buffer_size(),
            surface.target.as_mut(),
        ) else {
            return;
        };
        if pose.is_none() && surface.painted {
            return;
        }
        let frame = pose.zip(self.cursor.as_ref()).map(|(pose, cursor)| {
            ring::place(
                &self.style,
                pose,
                cursor.position,
                (logical.0 as f32, logical.1 as f32),
            )
        });
        if frame.is_some() {
            surface
                .layer
                .wl_surface()
                .frame(qh, FrameCallbackData(surface.layer.wl_surface().clone()));
            surface.frame_pending = true;
        }
        surface
            .viewport
            .set_destination(logical.0 as i32, logical.1 as i32);
        surface
            .layer
            .wl_surface()
            .damage_buffer(0, 0, size.0, size.1);
        if let Err(e) = self
            .gpu
            .draw(target, size, scale, frame.map(|f| (&self.style, f)))
        {
            eprintln!("wayrcursor: draw failed: {e:#}");
        }
        surface.painted = frame.is_none();
    }

    /// Starts the frame loop on the cursor's surface and clears any surface the ring left.
    fn on_cursor_changed(&mut self, qh: &QueueHandle<Self>) {
        if matches!(self.ring, RingState::Seeking { .. }) && self.cursor.is_some() {
            self.ring = RingState::Shown(Animation::new(self.timing, Instant::now()));
        }
        for index in 0..self.surfaces.len() {
            if !self.surfaces[index].frame_pending {
                self.render(qh, index);
            }
            if self.is_idle() {
                return;
            }
        }
    }
}

impl CompositorHandler for Overlay {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        wl_surface: &WlSurface,
        factor: i32,
    ) {
        if self.fractional.is_some() {
            return;
        }
        if let Some(index) = self.index_of(wl_surface) {
            self.surfaces[index].scale120 = factor.max(1) as u32 * 120;
            self.surfaces[index].painted = false;
            self.render(qh, index);
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: wayland_client::protocol::wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, wl_surface: &WlSurface, _: u32) {
        if let Some(index) = self.index_of(wl_surface) {
            self.surfaces[index].frame_pending = false;
            self.render(qh, index);
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }
}

impl LayerShellHandler for Overlay {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(index) = self.index_of(layer.wl_surface()) {
            let surface = self.surfaces.remove(index);
            if let Some(target) = surface.target {
                self.gpu.destroy_target(target);
            }
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(index) = self.index_of(layer.wl_surface()) else {
            return;
        };
        let surface = &mut self.surfaces[index];
        let first = surface.logical.is_none();
        surface.logical = Some(configure.new_size);
        surface.painted = false;
        if surface.target.is_none() {
            let size = surface.buffer_size().unwrap_or((1, 1));
            match self.gpu.create_target(layer.wl_surface(), size) {
                Ok(target) => surface.target = Some(target),
                Err(e) => {
                    eprintln!("wayrcursor: {e:#}");
                    return;
                }
            }
        }
        self.render(qh, index);
        if first {
            self.nudge_pointer(qh);
        }
    }
}

impl PointerHandler for Overlay {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        let mut changed = false;
        for event in events {
            if self.index_of(&event.surface).is_none() {
                continue;
            }
            let position = (event.position.0 as f32, event.position.1 as f32);
            match event.kind {
                PointerEventKind::Enter { serial } => {
                    if let Some((_, Some(device))) =
                        self.pointers.iter().find(|(p, _)| p == pointer)
                    {
                        device.set_shape(serial, Shape::Default);
                    }
                    self.cursor = Some(Cursor {
                        surface: event.surface.clone(),
                        position,
                    });
                    changed = true;
                }
                PointerEventKind::Leave { .. } => {
                    if self
                        .cursor
                        .as_ref()
                        .is_some_and(|c| c.surface == event.surface)
                    {
                        self.cursor = None;
                        changed = true;
                    }
                }
                PointerEventKind::Motion { .. } => {
                    if let Some(cursor) = &mut self.cursor {
                        cursor.position = position;
                    }
                }
                _ => {}
            }
        }
        if changed {
            self.on_cursor_changed(qh);
        }
    }
}

impl SeatHandler for Overlay {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability != Capability::Pointer {
            return;
        }
        if let Ok(pointer) = self.seats.get_pointer(qh, &seat) {
            let device = self
                .cursor_shape
                .as_ref()
                .map(|m| m.get_shape_device(&pointer, qh));
            self.pointers.push((pointer, device));
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability != Capability::Pointer {
            return;
        }
        self.pointers.retain(|(pointer, device)| {
            let keep = pointer
                .data::<smithay_client_toolkit::seat::pointer::PointerData<()>>()
                .is_none_or(|d| d.seat() != &seat);
            if !keep {
                if let Some(device) = device {
                    device.destroy();
                }
                pointer.release();
            }
            keep
        });
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl OutputHandler for Overlay {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        if let Some(index) = self.surfaces.iter().position(|s| s.output == output) {
            let surface = self.surfaces.remove(index);
            if let Some(target) = surface.target {
                self.gpu.destroy_target(target);
            }
        }
    }
}

impl ProvidesRegistryState for Overlay {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(Overlay);
smithay_client_toolkit::delegate_registry!(Overlay);

/// User data for protocol objects whose events carry nothing we use.
struct Ignore;

macro_rules! ignore_events {
    ($($proxy:ty),*) => {$(
        impl Dispatch2<$proxy, Overlay> for Ignore {
            fn event(&self, _: &mut Overlay, _: &$proxy, _: <$proxy as Proxy>::Event, _: &Connection, _: &QueueHandle<Overlay>) {}
        }
    )*};
}

ignore_events!(
    WpViewporter,
    WpViewport,
    WpFractionalScaleManagerV1,
    ZwlrVirtualPointerManagerV1,
    ZwlrVirtualPointerV1
);

struct ScaleOf(WlSurface);

impl Dispatch2<WpFractionalScaleV1, Overlay> for ScaleOf {
    fn event(
        &self,
        state: &mut Overlay,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &Connection,
        qh: &QueueHandle<Overlay>,
    ) {
        let wp_fractional_scale_v1::Event::PreferredScale { scale } = event else {
            return;
        };
        if let Some(index) = state.index_of(&self.0) {
            state.surfaces[index].scale120 = scale;
            state.surfaces[index].painted = false;
            state.render(qh, index);
        }
    }
}
