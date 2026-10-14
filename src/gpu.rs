use std::ffi::c_void;

use anyhow::{Context, anyhow};
use glow::HasContext;
use khronos_egl as egl;
use wayland_client::Proxy;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_egl::WlEglSurface;

use crate::config::{Rgba, RingStyle};
use crate::ring::{RingFrame, half_extent};

/// Polygon sides for the annulus mesh. The outer edge is circumscribed, so coverage is exact at any radius.
const SEGMENTS: u32 = 96;

const VERTEX: &str = r#"#version 100
attribute vec2 a_unit;
uniform vec2 u_center;
uniform float u_inner;
uniform float u_outer;
uniform vec2 u_size;
varying vec2 v_pos;
void main() {
    float angle = a_unit.x;
    float r = mix(u_inner, u_outer, a_unit.y);
    v_pos = u_center + vec2(cos(angle), sin(angle)) * r;
    vec2 ndc = v_pos / u_size * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"#version 100
precision highp float;
uniform vec2 u_center;
uniform float u_radius;
uniform float u_core;
uniform float u_edge;
uniform float u_px;
uniform float u_alpha;
uniform vec4 u_color;
uniform vec4 u_outline;
varying vec2 v_pos;
float coverage(float half_width, float dist) {
    return clamp((half_width - dist) / u_px + 0.5, 0.0, 1.0);
}
void main() {
    float dist = abs(length(v_pos - u_center) - u_radius);
    float core = coverage(u_core, dist) * u_color.a;
    float edge = coverage(u_edge, dist) * u_outline.a * (1.0 - core);
    gl_FragColor = vec4(u_color.rgb * core + u_outline.rgb * edge, core + edge) * u_alpha;
}
"#;

pub struct Gpu {
    egl: egl::DynamicInstance<egl::EGL1_4>,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
    gl: glow::Context,
    program: glow::Program,
    mesh: glow::Buffer,
}

/// A window-sized EGL surface backing one overlay.
pub struct Target {
    window: WlEglSurface,
    surface: egl::Surface,
    size: (i32, i32),
}

impl Gpu {
    pub fn new(display_ptr: *mut c_void) -> anyhow::Result<Self> {
        let egl = unsafe { egl::DynamicInstance::<egl::EGL1_4>::load_required() }
            .map_err(|e| anyhow!("loading libEGL: {e}"))?;
        let display = unsafe { egl.get_display(display_ptr) }.context("no EGL display")?;
        egl.initialize(display)?;
        egl.bind_api(egl::OPENGL_ES_API)?;
        let attributes = [
            egl::SURFACE_TYPE,
            egl::WINDOW_BIT,
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES2_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::NONE,
        ];
        let config = egl
            .choose_first_config(display, &attributes)?
            .context("no RGBA8 GLES2 EGL config")?;
        let context = egl.create_context(
            display,
            config,
            None,
            &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE],
        )?;
        egl.make_current(display, None, None, Some(context))?;
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name)
                    .map_or(std::ptr::null(), |f| f as *const c_void)
            })
        };
        let (program, mesh) = unsafe { build_pipeline(&gl)? };
        Ok(Self {
            egl,
            display,
            config,
            context,
            gl,
            program,
            mesh,
        })
    }

    pub fn create_target(&self, surface: &WlSurface, size: (i32, i32)) -> anyhow::Result<Target> {
        let window = WlEglSurface::new(surface.id(), size.0, size.1)?;
        let surface = unsafe {
            self.egl.create_window_surface(
                self.display,
                self.config,
                window.ptr() as egl::NativeWindowType,
                None,
            )?
        };
        Ok(Target {
            window,
            surface,
            size,
        })
    }

    pub fn destroy_target(&self, target: Target) {
        let _ = self.egl.destroy_surface(self.display, target.surface);
        drop(target.window);
    }

    /// Draws one frame and presents it. `scale` maps logical pixels to buffer pixels.
    pub fn draw(
        &self,
        target: &mut Target,
        size: (i32, i32),
        scale: f32,
        ring: Option<(&RingStyle, RingFrame)>,
    ) -> anyhow::Result<()> {
        if target.size != size {
            target.window.resize(size.0, size.1, 0, 0);
            target.size = size;
        }
        self.egl.make_current(
            self.display,
            Some(target.surface),
            Some(target.surface),
            Some(self.context),
        )?;
        self.egl.swap_interval(self.display, 0)?;
        let gl = &self.gl;
        unsafe {
            gl.viewport(0, 0, size.0, size.1);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            if let Some((style, frame)) = ring {
                self.draw_ring(size, scale, style, frame);
            }
        }
        self.egl.swap_buffers(self.display, target.surface)?;
        Ok(())
    }

    unsafe fn draw_ring(&self, size: (i32, i32), scale: f32, style: &RingStyle, frame: RingFrame) {
        let gl = &self.gl;
        let extent = half_extent(style);
        let set1 = |name: &str, v: f32| unsafe {
            gl.uniform_1_f32(gl.get_uniform_location(self.program, name).as_ref(), v)
        };
        let set2 = |name: &str, a: f32, b: f32| unsafe {
            gl.uniform_2_f32(gl.get_uniform_location(self.program, name).as_ref(), a, b)
        };
        let set4 = |name: &str, c: Rgba| unsafe {
            gl.uniform_4_f32(
                gl.get_uniform_location(self.program, name).as_ref(),
                c.r,
                c.g,
                c.b,
                c.a,
            )
        };
        unsafe {
            gl.use_program(Some(self.program));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.mesh));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 0, 0);
        }
        set2("u_center", frame.center.0, frame.center.1);
        set2("u_size", size.0 as f32 / scale, size.1 as f32 / scale);
        set1("u_inner", (frame.radius - extent).max(0.0));
        set1(
            "u_outer",
            (frame.radius + extent) / (std::f32::consts::PI / SEGMENTS as f32).cos(),
        );
        set1("u_radius", frame.radius);
        set1("u_core", style.width / 2.0);
        set1("u_edge", style.width / 2.0 + style.outline_width);
        set1("u_px", 1.0 / scale);
        set1("u_alpha", frame.alpha);
        set4("u_color", style.color);
        set4("u_outline", style.outline);
        unsafe {
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 2 * (SEGMENTS as i32 + 1));
        }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        let _ = self.egl.make_current(self.display, None, None, None);
        let _ = self.egl.destroy_context(self.display, self.context);
        let _ = self.egl.terminate(self.display);
    }
}

unsafe fn build_pipeline(gl: &glow::Context) -> anyhow::Result<(glow::Program, glow::Buffer)> {
    unsafe {
        let program = gl.create_program().map_err(|e| anyhow!(e))?;
        for (kind, source) in [
            (glow::VERTEX_SHADER, VERTEX),
            (glow::FRAGMENT_SHADER, FRAGMENT),
        ] {
            let shader = gl.create_shader(kind).map_err(|e| anyhow!(e))?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                return Err(anyhow!("shader: {}", gl.get_shader_info_log(shader)));
            }
            gl.attach_shader(program, shader);
            gl.delete_shader(shader);
        }
        gl.bind_attrib_location(program, 0, "a_unit");
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            return Err(anyhow!("link: {}", gl.get_program_info_log(program)));
        }

        let step = std::f32::consts::TAU / SEGMENTS as f32;
        let vertices: Vec<f32> = (0..=SEGMENTS)
            .flat_map(|i| {
                let angle = i as f32 * step;
                [angle, 0.0, angle, 1.0]
            })
            .collect();
        let mesh = gl.create_buffer().map_err(|e| anyhow!(e))?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(mesh));
        let bytes = std::slice::from_raw_parts(vertices.as_ptr().cast::<u8>(), vertices.len() * 4);
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
        Ok((program, mesh))
    }
}
