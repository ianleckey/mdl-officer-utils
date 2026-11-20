mod addon;
mod api;
mod art;
mod config;
mod data_addon;
mod eventlog;
mod game;
mod lfg;
mod lua;
mod report;
mod savedvars;
mod schedule;
mod tui;
mod update;
mod version;
mod worker;

use std::fs::{File, OpenOptions};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, mpsc};

const HELP: &str = "\
mdl-desktop: the Murloc Defence League officers' desktop app.

Run it with no arguments to open the dashboard.

Options:
  --version   print the version and exit
  --help      print this and exit";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("--version" | "-V") => {
            println!("mdl-desktop {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some("--help" | "-h") => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("Unknown option `{other}`.\n\n{HELP}");
            return ExitCode::FAILURE;
        }
    }

    // One copy at a time: two would both upload, and (phase 2) both write Data.lua.
    // The lock goes when this handle is dropped at the end of main, or by the OS
    // if the app crashes, so it can never get stuck.
    let _lock = match take_lock() {
        Ok(lock) => lock,
        Err(LockError::Taken) => {
            eprintln!("The MDL desktop app is already running in another window.");
            return ExitCode::FAILURE;
        }
        Err(LockError::Io(e)) => {
            // Couldn't even create the lock file (odd permissions?). Run anyway.
            eprintln!("Couldn't create the lock file ({e}); carrying on.");
            None
        }
    };

    let config = config::Config::load();

    // The dashboard and the worker share `Status`. The worker writes it; the
    // dashboard redraws from it a few times a second.
    let status = Arc::new(Mutex::new(worker::Status::default()));
    // The dashboard sends `Command`s to the worker (new settings, upload now, quit).
    let (tx, rx) = mpsc::channel();

    let config = match config {
        Ok(c) => c,
        Err(e) => {
            // Can't print: the dashboard is about to take over the screen. Log it instead.
            let event = eventlog::Event::now(
                eventlog::Level::Error,
                format!("Couldn't read the settings ({e:#}); starting with defaults"),
            );
            if let Ok(mut s) = status.lock() {
                s.push_event(event);
            }
            config::Config::default()
        }
    };

    worker::spawn(config.clone(), status.clone(), rx);
    update::spawn(status.clone());
    match tui::run(config, status, tx) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("The dashboard stopped: {e}");
            ExitCode::FAILURE
        }
    }
}

enum LockError {
    Taken,
    Io(std::io::Error),
}

/// Locks `mdl-desktop.lock` in the app's config folder. `Ok(None)` if there's no
/// config folder at all (nothing to lock against).
fn take_lock() -> Result<Option<File>, LockError> {
    let Ok(dir) = config::Config::dir() else { return Ok(None) };
    std::fs::create_dir_all(&dir).map_err(LockError::Io)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("mdl-desktop.lock"))
        .map_err(LockError::Io)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err(LockError::Taken),
        Err(std::fs::TryLockError::Error(e)) => Err(LockError::Io(e)),
    }
}
