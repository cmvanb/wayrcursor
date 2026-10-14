mod anim;
mod config;
mod daemon;
mod gpu;
mod input;
mod overlay;
mod ring;
mod shake;

use clap::{Args, Parser, Subcommand};

use crate::config::Config;
use crate::overlay::Session;

/// Shows where the cursor is with a ring that closes in on it.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the ring once. A running daemon draws it; otherwise this process does.
    Locate(Style),
    /// Stay running: show the ring when the pointer is shaken or `locate` asks.
    Daemon(Style),
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
    match Cli::parse().command {
        Command::Locate(style) => {
            let overrides = style.overrides();
            let config = Config::load(&style.config, &overrides)?;
            if daemon::request_locate(&overrides)? {
                return Ok(());
            }
            let mut session = Session::connect()?;
            session.overlay.trigger(&session.qh, &config);
            while !session.overlay.is_idle() {
                session.step()?;
            }
            Ok(())
        }
        Command::Daemon(style) => daemon::run(style.config.clone(), style.overrides()),
    }
}
