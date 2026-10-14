mod anim;
mod config;
mod gpu;
mod overlay;
mod ring;
mod shake;

use std::time::Instant;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use smithay_client_toolkit::reexports::{
    calloop::EventLoop, calloop_wayland_source::WaylandSource,
};
use wayland_client::{Connection, globals::registry_queue_init};

use crate::config::Config;
use crate::overlay::Overlay;

/// Shows where the cursor is with a ring that closes in on it.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the ring once.
    Locate(Style),
}

/// Per-invocation overrides of the config file.
#[derive(Args)]
struct Style {
    /// Ring color, #rrggbb or #rrggbbaa.
    #[arg(long)]
    color: Option<String>,
    /// Outline color on both ring edges, #rrggbb or #rrggbbaa.
    #[arg(long)]
    outline: Option<String>,
    /// Ring width in logical pixels.
    #[arg(long)]
    width: Option<f64>,
    /// Outline width in logical pixels.
    #[arg(long)]
    outline_width: Option<f64>,
    /// Radius the ring settles at around the cursor.
    #[arg(long)]
    radius: Option<f64>,
    /// Config file path.
    #[arg(long, default_value_os_t = config::default_path())]
    config: std::path::PathBuf,
}

impl Style {
    fn overrides(&self) -> toml::Table {
        let mut ring = toml::Table::new();
        let mut set = |key: &str, value: Option<toml::Value>| {
            if let Some(value) = value {
                ring.insert(key.into(), value);
            }
        };
        set("color", self.color.clone().map(Into::into));
        set("outline", self.outline.clone().map(Into::into));
        set("width", self.width.map(Into::into));
        set("outline_width", self.outline_width.map(Into::into));
        set("radius", self.radius.map(Into::into));
        let mut table = toml::Table::new();
        if !ring.is_empty() {
            table.insert("ring".into(), ring.into());
        }
        table
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Locate(style) => locate(Config::load(&style.config, &style.overrides())?),
    }
}

fn locate(config: Config) -> anyhow::Result<()> {
    let conn = Connection::connect_to_env().context("connecting to the Wayland compositor")?;
    let (globals, queue) = registry_queue_init(&conn)?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<Overlay> = EventLoop::try_new()?;
    WaylandSource::new(conn.clone(), queue)
        .insert(event_loop.handle())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut overlay = Overlay::new(&conn, &globals, &qh)?;
    event_loop.dispatch(std::time::Duration::ZERO, &mut overlay)?;
    overlay.trigger(&qh, &config);
    while !overlay.is_idle() {
        let timeout = overlay
            .wake_at()
            .map(|t| t.saturating_duration_since(Instant::now()));
        event_loop.dispatch(timeout, &mut overlay)?;
        overlay.tick();
    }
    conn.flush()?;
    Ok(())
}
