use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;

use crate::anim::Timing;

/// Straight (non-premultiplied) RGBA, each channel in 0..=1.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl TryFrom<String> for Rgba {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let hex = text.strip_prefix('#').unwrap_or(&text);
        let invalid = || format!("invalid color {text:?}: expected #rrggbb or #rrggbbaa");
        if !matches!(hex.len(), 6 | 8) || !hex.is_ascii() {
            return Err(invalid());
        }
        let channel = |i: usize| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map(|v| v as f32 / 255.0)
                .map_err(|_| invalid())
        };
        let a = if hex.len() == 8 { channel(6)? } else { 1.0 };
        Ok(Self {
            r: channel(0)?,
            g: channel(2)?,
            b: channel(4)?,
            a,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RingStyle {
    pub width: f32,
    pub radius: f32,
    pub color: Rgba,
    pub outline: Rgba,
    pub outline_width: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShakeParams {
    pub reversals: u32,
    pub window_ms: u64,
    pub min_travel_mm: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TimingMs {
    inbound_ms: u64,
    hold_ms: u64,
    fade_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub ring: RingStyle,
    timing: TimingMs,
    pub shake: ShakeParams,
}

impl Default for RingStyle {
    fn default() -> Self {
        Self {
            width: 6.0,
            radius: 40.0,
            color: Rgba {
                r: 1.0,
                g: 0.76,
                b: 0.03,
                a: 1.0,
            },
            outline: Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.7,
            },
            outline_width: 1.0,
        }
    }
}

impl Default for ShakeParams {
    fn default() -> Self {
        Self {
            reversals: 4,
            window_ms: 600,
            min_travel_mm: 8.0,
        }
    }
}

impl Default for TimingMs {
    fn default() -> Self {
        Self {
            inbound_ms: 350,
            hold_ms: 150,
            fade_ms: 250,
        }
    }
}

impl Config {
    pub fn timing(&self) -> Timing {
        Timing {
            inbound: Duration::from_millis(self.timing.inbound_ms),
            hold: Duration::from_millis(self.timing.hold_ms),
            fade: Duration::from_millis(self.timing.fade_ms),
        }
    }

    /// Layers `overrides` (a partial config table) over the file's contents.
    pub fn parse(file: &str, overrides: &toml::Table) -> anyhow::Result<Self> {
        let mut table: toml::Table = file.parse()?;
        merge(&mut table, overrides);
        Ok(toml::Value::Table(table).try_into()?)
    }

    /// Reads the config file if present; a missing file means defaults.
    pub fn load(path: &Path, overrides: &toml::Table) -> anyhow::Result<Self> {
        let file = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Self::parse(&file, overrides).with_context(|| format!("in {}", path.display()))
    }
}

pub fn merge(base: &mut toml::Table, overlay: &toml::Table) {
    for (key, value) in overlay {
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("wayrcursor").join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> toml::Table {
        text.parse().unwrap()
    }

    #[test]
    fn empty_file_yields_defaults() {
        let config = Config::parse("", &toml::Table::new()).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.ring.width, 6.0);
        assert_eq!(config.timing().inbound, Duration::from_millis(350));
    }

    #[test]
    fn file_sets_only_the_keys_it_names() {
        let config = Config::parse(
            "[ring]\nwidth = 10\ncolor = \"#ff000080\"\n",
            &toml::Table::new(),
        )
        .unwrap();
        assert_eq!(config.ring.width, 10.0);
        assert_eq!(
            config.ring.color,
            Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 128.0 / 255.0
            }
        );
        assert_eq!(config.ring.radius, 40.0);
    }

    #[test]
    fn overrides_beat_the_file_without_erasing_siblings() {
        let file = "[ring]\nwidth = 10\nradius = 20\n";
        let config = Config::parse(file, &table("ring = { width = 3 }")).unwrap();
        assert_eq!(config.ring.width, 3.0);
        assert_eq!(config.ring.radius, 20.0);
    }

    #[test]
    fn colors_accept_rgb_and_rgba_hex() {
        assert_eq!(
            Rgba::try_from("#00ff00".to_string()),
            Ok(Rgba {
                r: 0.0,
                g: 1.0,
                b: 0.0,
                a: 1.0
            })
        );
        assert_eq!(
            Rgba::try_from("0000FF00".to_string()),
            Ok(Rgba {
                r: 0.0,
                g: 0.0,
                b: 1.0,
                a: 0.0
            })
        );
        assert!(Rgba::try_from("#12345".to_string()).is_err());
        assert!(Rgba::try_from("#gg0000".to_string()).is_err());
    }

    #[test]
    fn typos_are_rejected() {
        let err = Config::parse("[ring]\nwdith = 3\n", &toml::Table::new()).unwrap_err();
        assert!(format!("{err:#}").contains("wdith"), "{err:#}");
    }
}
