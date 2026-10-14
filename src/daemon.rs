use std::cell::RefCell;
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use anyhow::Context;
use inotify::{Inotify, WatchMask};
use smithay_client_toolkit::reexports::calloop::{Interest, Mode, PostAction, generic::Generic};

use crate::config::{self, Config};
use crate::input;
use crate::overlay::Session;
use crate::shake::ShakeDetector;

/// The daemon's socket, one per Wayland display so nested compositors get their own.
pub fn socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let display = std::env::var_os("WAYLAND_DISPLAY").unwrap_or_else(|| "wayland-0".into());
    let display = Path::new(&display)
        .file_name()
        .unwrap_or(OsStr::new("wayland-0"))
        .to_string_lossy();
    runtime.join(format!("wayrcursor-{display}.sock"))
}

/// Hands a locate request to a running daemon. Returns false when none is listening.
pub fn request_locate(overrides: &toml::Table) -> anyhow::Result<bool> {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else {
        return Ok(false);
    };
    stream.write_all(overrides.to_string().as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    Ok(true)
}

struct Settings {
    path: PathBuf,
    overrides: toml::Table,
    config: Config,
}

impl Settings {
    fn with_request(&self, request: &toml::Table) -> anyhow::Result<Config> {
        let mut overrides = self.overrides.clone();
        config::merge(&mut overrides, request);
        Config::load(&self.path, &overrides)
    }

    fn reload(&mut self) {
        match Config::load(&self.path, &self.overrides) {
            Ok(config) => self.config = config,
            Err(e) => eprintln!("wayrcursor: keeping previous config: {e:#}"),
        }
    }
}

pub fn run(path: PathBuf, overrides: toml::Table) -> anyhow::Result<()> {
    let config = Config::load(&path, &overrides)?;
    let settings = Rc::new(RefCell::new(Settings {
        path,
        overrides,
        config,
    }));
    let mut session = Session::connect()?;
    let handle = session.event_loop.handle();
    let qh = session.qh.clone();

    let listener = bind(&socket_path())?;
    listener.set_nonblocking(true)?;
    let requests = settings.clone();
    let request_qh = qh.clone();
    handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            move |_, listener, overlay| {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => match read_request(stream)
                            .and_then(|r| requests.borrow().with_request(&r))
                        {
                            Ok(config) => overlay.trigger(&request_qh, &config),
                            Err(e) => eprintln!("wayrcursor: bad locate request: {e:#}"),
                        },
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            return Ok(PostAction::Continue);
                        }
                        Err(e) => return Err(e),
                    }
                }
            },
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    watch_config(&session, settings.clone())?;

    let detector = RefCell::new(ShakeDetector::new(settings.borrow().config.shake));
    let shake_settings = settings.clone();
    input::watch(&handle, move |motion, overlay| {
        let settings = shake_settings.borrow();
        let mut detector = detector.borrow_mut();
        detector.set_params(settings.config.shake);
        if detector.feed(motion) {
            overlay.trigger(&qh, &settings.config);
        }
    })
    .context("watching input devices")?;

    loop {
        session.step()?;
    }
}

/// Binds the socket, replacing a stale one left by a daemon that died.
fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    if UnixStream::connect(path).is_ok() {
        anyhow::bail!(
            "a wayrcursor daemon is already running on {}",
            path.display()
        );
    }
    let _ = std::fs::remove_file(path);
    UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))
}

fn read_request(stream: UnixStream) -> anyhow::Result<toml::Table> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    let mut text = String::new();
    stream.take(64 * 1024).read_to_string(&mut text)?;
    Ok(text.parse()?)
}

/// Reloads the config whenever its file is written, replaced, or removed.
fn watch_config(session: &Session, settings: Rc<RefCell<Settings>>) -> anyhow::Result<()> {
    let path = settings.borrow().path.clone();
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(());
    };
    let name = name.to_owned();
    std::fs::create_dir_all(dir).ok();
    let inotify = Inotify::init()?;
    if inotify
        .watches()
        .add(
            dir,
            WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO | WatchMask::CREATE | WatchMask::DELETE,
        )
        .is_err()
    {
        eprintln!("wayrcursor: not watching {} for changes", dir.display());
        return Ok(());
    }
    session
        .event_loop
        .handle()
        .insert_source(
            Generic::new(inotify, Interest::READ, Mode::Level),
            move |_, inotify, _| {
                let mut buffer = [0; 4096];
                let touched = match unsafe { inotify.get_mut() }.read_events(&mut buffer) {
                    Ok(mut events) => events.any(|e| e.name == Some(name.as_os_str())),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => false,
                    Err(e) => return Err(e),
                };
                if touched {
                    settings.borrow_mut().reload();
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}
