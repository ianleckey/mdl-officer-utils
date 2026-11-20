//! The background thread that does the actual work.
//!
//! The dashboard (main thread) never waits on the network or the disk. It sends
//! `Command`s here and redraws from the shared `Status` a few times a second.
//!
//! The worker is also the only thing that saves config.toml, so the dashboard and
//! the worker can never overwrite each other's changes (the token versus the export hashes).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::addon::{self, AddonView};
use crate::api::{self, ApiError, ImportResult};
use crate::config::Config;
use crate::eventlog::{self, Event, Level};
use crate::game::{self, SavedVarFile};
use crate::lfg::{self, Acks, LfgSnapshot, Run};
use crate::report::{self, Report};
use crate::update::UpdateInfo;
use crate::version::Version;
use crate::{data_addon, savedvars};

/// How often to look at the saved-data files' modified times (the spec says 30 seconds).
pub const CHECK_EVERY: Duration = Duration::from_secs(30);
/// Backoff ceiling for 5xx and network errors (the spec says 15 minutes).
pub const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// Phase 2: how often to ask the website for runs (the spec's default: 5 minutes).
pub const LFG_POLL_EVERY: Duration = Duration::from_secs(5 * 60);
/// Phase 2: never poll /lfg more often than this (the spec: at most once a minute).
pub const LFG_MIN_GAP: Duration = Duration::from_secs(60);

/// Messages from the dashboard.
#[derive(Debug)]
pub enum Command {
    /// The officer changed the token or folder. The worker stores these in config.toml and starts over.
    SettingsChanged { token: String, game_dir: Option<PathBuf> },
    /// The `u` key: check the files now instead of waiting for the next check.
    UploadNow,
    /// The officer quit.
    Quit,
}

/// What the app is doing, for the coloured badge at the top of the dashboard.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No token, or no game folder.
    #[default]
    SetupNeeded,
    CheckingToken,
    Watching,
    Uploading,
    /// Backing off after the website couldn't be reached (see `Status::next_check`).
    Retrying,
    /// The website said "too many requests" (see `Status::next_check`).
    RateLimited,
    /// 401/403: nothing happens until the token changes.
    Paused,
}

/// What we last found in one account's saved-data file.
#[derive(Debug, Clone, PartialEq)]
pub enum AccountState {
    /// Not looked at yet.
    Waiting,
    /// The file has no `murlocExport`: this addon version doesn't send figures.
    NoExport,
    /// The export is the same one already sent (maybe by an earlier run of the app).
    UpToDate,
    /// Sent this session. `changed` is the website's answer: false means it already had them.
    Uploaded { at: SystemTime, changed: bool },
    /// The website refused the export (422); it won't be resent until it changes.
    Rejected(String),
    /// The file couldn't be read or parsed (often: the game was mid-write; retried next check).
    ReadError(String),
}

#[derive(Debug, Clone)]
pub struct AccountView {
    pub name: String,
    pub state: AccountState,
}

/// What the dashboard shows. The worker writes it; the dashboard only reads it.
#[derive(Debug, Default, Clone)]
pub struct Status {
    pub phase: Phase,
    /// From GET /me, once the token checks out.
    pub signed_in_as: Option<String>,
    /// The game folder in use, and the account folders found under it.
    pub game_dir: Option<PathBuf>,
    pub accounts: Vec<AccountView>,
    /// The last successful POST /murlocs.
    pub last_upload: Option<Upload>,
    /// The most recent problem, in words the officer can act on. Cleared after a success.
    pub last_error: Option<String>,
    /// True after a 401/403: nothing happens until the settings change.
    pub paused: bool,
    /// A short "what am I doing" line, e.g. "Watching 2 accounts".
    pub activity: String,
    /// When the next automatic check happens (for the countdown).
    pub next_check: Option<Instant>,
    /// Newest last. At most `eventlog::KEEP`.
    pub events: VecDeque<Event>,
    /// Phase 2: the group finder panel. `None` while `lfg_enabled` is off.
    pub lfg: Option<LfgView>,
    /// A newer release of this app (from the update-check thread).
    pub update: Option<UpdateInfo>,
    /// The installed addon's version, and what the website says (step 13).
    pub addon: Option<AddonView>,
    /// The newest Impact Report export read from any account (step 11).
    pub report: Option<Report>,
    /// Why there's no report to show (e.g. "step 11 … isn't written yet").
    pub report_note: Option<String>,
}

/// Phase 2: what the dashboard's Group finder panel shows. Built by `Worker::publish_lfg`.
#[derive(Debug, Default, Clone)]
pub struct LfgView {
    /// How many runs the last `GET /lfg` returned.
    pub runs_known: usize,
    /// The next few runs, soonest first.
    pub upcoming: Vec<Run>,
    /// Sign-ups the website has answered this session.
    pub signups_sent: usize,
    /// Sign-ups read from an outbox but not answered yet (they're retried).
    pub pending: usize,
    /// When `GET /lfg` last answered (200 or 304).
    pub last_poll: Option<SystemTime>,
}

impl Status {
    /// Adds an event to the Events panel and to events.log.
    pub(crate) fn push_event(&mut self, event: Event) {
        eventlog::append(&event);
        self.events.push_back(event);
        while self.events.len() > eventlog::KEEP {
            self.events.pop_front();
        }
    }
}

#[derive(Debug, Clone)]
pub struct Upload {
    pub at: SystemTime,
    pub account: String,
    pub result: ImportResult,
}

/// Starts the worker thread. Called once, from main.rs.
pub fn spawn(config: Config, status: Arc<Mutex<Status>>, rx: Receiver<Command>) {
    std::thread::Builder::new()
        .name("worker".into())
        .spawn(move || {
            let mut w = Worker {
                config,
                status,
                client: None,
                files: Vec::new(),
                last_seen: HashMap::new(),
                retry_after: None,
                backoff: None,
                next_check: Instant::now(),
                lfg: LfgState::default(),
            };
            w.event(Level::Info, "App started");
            w.start();
            w.run(rx);
        })
        .expect("spawning the worker thread");
}

/// Phase 2: everything the worker keeps for the group finder.
#[derive(Default)]
struct LfgState {
    /// The last runs from `GET /lfg` (also what's in Data.lua).
    snapshot: Option<LfgSnapshot>,
    /// The website's answers, keyed by client_id. Saved in Data.lua, read back by `lfg_start`.
    acks: Acks,
    /// Outbox items read but not answered yet (a transient error); resent next time.
    pending: Vec<serde_json::Value>,
    /// The `## Interface:` number for the data addon's .toc.
    interface: String,
    /// When to call `poll_lfg` next. `None` = not scheduled (switch off, or not started).
    next_poll: Option<Instant>,
    /// Counters for the dashboard.
    signups_sent: usize,
    last_poll: Option<SystemTime>,
}

struct Worker {
    config: Config,
    status: Arc<Mutex<Status>>,

    /// Built in `start()` once there's a token.
    client: Option<api::Client>,
    /// The saved-data files found in `start()`.
    files: Vec<SavedVarFile>,
    /// Each file's modified time when we last read it, so we only reread on change.
    last_seen: HashMap<PathBuf, SystemTime>,
    /// Set after a 429: don't call the site again before this moment.
    retry_after: Option<Instant>,
    /// Current backoff after 5xx/network errors (`None` means all is well).
    backoff: Option<Duration>,
    /// When `run` should next call `tick`.
    next_check: Instant,
    /// Phase 2 state (unused while `config.lfg_enabled` is off).
    lfg: LfgState,
}

impl Worker {
    /// The main loop. It sleeps until either a command arrives or it's time for the next check.
    fn run(&mut self, rx: Receiver<Command>) {
        loop {
            // Sleep until the next murloc check or the next /lfg poll, whichever is first.
            let due = match self.lfg.next_poll {
                Some(poll) => poll.min(self.next_check),
                None => self.next_check,
            };
            let wait = due.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(Command::Quit) | Err(RecvTimeoutError::Disconnected) => {
                    self.event(Level::Info, "App stopped");
                    return;
                }
                Ok(Command::SettingsChanged { token, game_dir }) => {
                    self.config.token = token;
                    self.config.game_dir = game_dir;
                    self.save_config();
                    self.event(Level::Info, "Settings saved; starting over");
                    self.start();
                }
                Ok(Command::UploadNow) => {
                    self.last_seen.clear(); // Forget the modified times, so every file is reread.
                    self.event(Level::Info, "Checking now (asked for)");
                    self.tick();
                }
                Err(RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    if self.lfg.next_poll.is_some_and(|t| t <= now) {
                        self.poll_lfg();
                        // Safety net for the spec's "at most once a minute": if poll_lfg
                        // didn't schedule a later poll, do it here so we never spin.
                        if self.lfg.next_poll.is_some_and(|t| t <= now) {
                            self.lfg.next_poll = Some(now + LFG_MIN_GAP);
                        }
                    }
                    if self.next_check <= now {
                        self.tick();
                    }
                }
            }
        }
    }

    /// Change the status. Anything that changed in `activity`, `last_error` or
    /// `last_upload` is also recorded as an event (Events panel + events.log).
    /// Use it like: `self.set_status(|s| s.activity = "Checking token…".into());`
    fn set_status(&self, f: impl FnOnce(&mut Status)) {
        let Ok(mut s) = self.status.lock() else { return };
        let before = (s.activity.clone(), s.last_error.clone(), s.last_upload.as_ref().map(|u| u.at));
        f(&mut s);
        if s.activity != before.0 && !s.activity.is_empty() {
            let text = s.activity.clone();
            s.push_event(Event::now(Level::Info, text));
        }
        if s.last_error != before.1 {
            if let Some(e) = s.last_error.clone() {
                s.push_event(Event::now(Level::Error, e));
            }
        }
        if let Some(u) = s.last_upload.clone() {
            if Some(u.at) != before.2 {
                let what = if u.result.changed { "new figures" } else { "no change (the website already had them)" };
                s.push_event(Event::now(Level::Ok, format!("Uploaded {}: {what}", u.account)));
            }
        }
    }

    /// Records an event that isn't a status change (a warning, say).
    fn event(&self, level: Level, text: impl Into<String>) {
        if let Ok(mut s) = self.status.lock() {
            s.push_event(Event::now(level, text));
        }
    }

    /// Updates one account's row. Returns its previous state.
    fn set_account(&self, account: &str, state: AccountState) -> Option<AccountState> {
        let mut s = self.status.lock().ok()?;
        let row = s.accounts.iter_mut().find(|a| a.name == account)?;
        Some(std::mem::replace(&mut row.state, state))
    }

    /// Sets the badge, and publishes when the next check is due.
    fn set_phase(&self, phase: Phase) {
        let next = self.next_check;
        self.set_status(|s| {
            s.phase = phase;
            s.next_check = Some(next);
        });
    }

    /// Writes config.toml, reporting a failure.
    fn save_config(&self) {
        if let Err(e) = self.config.save() {
            self.set_status(|s| s.last_error = Some(format!("Couldn't save settings: {e:#}")));
        }
    }

    /// Runs at launch and after every token or folder change.
    ///
    /// 1. Forget old state: client, files, last_seen, retry_after, backoff, and paused in the status.
    /// 2. No token yet? Say so in `activity` and stop here.
    /// 3. Build an `api::Client` and call `me()`. Not an officer, or 401/403?
    ///    Set `paused`, show why, stop. Otherwise put the name in `signed_in_as`.
    /// 4. Work out the game folder: `config.game_dir`, or else `game::detect()`.
    ///    Then `game::saved_var_files()`. Put both in the status.
    /// 5. Set `next_check` to now, so the first `tick` runs straight away.
    fn start(&mut self) {
        self.client = None;
        self.retry_after = None;
        self.backoff = None;
        self.files.clear();
        self.last_seen.clear();
        self.set_status(|s| {
            s.paused = false;
            s.signed_in_as = None;
            s.last_error = None;
            s.accounts.clear();
        });
        self.next_check = Instant::now() + CHECK_EVERY;

        if self.config.token.is_empty() {
            self.set_phase(Phase::SetupNeeded);
            self.set_status(|s| s.activity = "No token yet: press t and paste one from the website".to_string());
            return;
        }

        self.set_phase(Phase::CheckingToken);
        let client = api::Client::new(&self.config.api_base, &self.config.token);

        match client.me() {
            Ok(me) if me.officer => {
                self.event(Level::Ok, format!("Signed in as {}", me.name));
                self.set_status(|s| s.signed_in_as = Some(me.name));
                self.client = Some(client);
            }
            Ok(_) => {
                self.set_phase(Phase::Paused);
                self.set_status(|s| {
                    s.paused = true;
                    s.last_error = Some(ApiError::NotOfficer.to_string());
                });
                return;
            }
            Err(e @ (ApiError::Unauthorized | ApiError::NotOfficer)) => {
                self.set_phase(Phase::Paused);
                self.set_status(|s| {
                    s.paused = true;
                    s.last_error = Some(e.to_string());
                });
                return;
            }
            Err(e) => {
                self.set_status(|s| s.last_error = Some(e.to_string()));
                self.client = Some(client);
            }
        }

        let Some(dir) = self.config.game_dir.clone().or_else(game::detect) else {
            self.set_phase(Phase::SetupNeeded);
            self.set_status(|s| s.activity = "No game folder: press d to detect it, or f to type it".to_string());
            return;
        };

        match game::saved_var_files(&dir) {
            Ok(files) => {
                let accounts = files
                    .iter()
                    .map(|f| AccountView { name: f.account.clone(), state: AccountState::Waiting })
                    .collect();
                self.set_status(|s| {
                    s.game_dir = Some(dir.clone());
                    s.accounts = accounts;
                });
                if files.is_empty() {
                    self.event(
                        Level::Warn,
                        "No account has MurlocDefenceLeague.lua yet: log in with the addon enabled, then /reload",
                    );
                }
                self.files = files;
            }
            Err(e) => {
                self.set_phase(Phase::SetupNeeded);
                self.set_status(|s| s.last_error = Some(format!("Couldn't read the game folder: {e}")));
                return;
            }
        }

        self.check_addon(&dir);

        self.next_check = Instant::now();
        self.set_phase(Phase::Watching);

        self.lfg = LfgState::default();
        if self.config.lfg_enabled {
            self.lfg_start();
            self.lfg.next_poll = Some(Instant::now());
        }
        self.publish_lfg();
    }

    /// Runs every `CHECK_EVERY` (and when `u` is pressed).
    ///
    /// 1. Paused, or still inside a 429 wait? Just schedule the next check.
    /// 2. For each file in `self.files`: compare its modified time with `last_seen`.
    ///    Unchanged → skip it.
    /// 3. Read it, then `savedvars::read_key(text, "murlocExport")`. `None` → skip it.
    /// 4. SHA-256 the text. Same as `config.export_hashes[account]` → skip it.
    /// 5. `post_murlocs`, then:
    ///      Ok          → save the hash, record the upload, clear the error, reset backoff
    ///      Rejected    → save the hash anyway, show the message
    ///      Unauthorized / NotOfficer → paused
    ///      RateLimited → set retry_after
    ///      Transient   → DON'T save the hash or last_seen; grow the backoff (×2, max 15 min)
    /// 6. Set `next_check`: now + backoff if backing off, else now + CHECK_EVERY.
    fn tick(&mut self) {
        let paused = self.status.lock().map(|s| s.paused).unwrap_or(false);
        let waiting_out_429 = self.retry_after.is_some_and(|t| Instant::now() < t);

        // Nothing to do until the token is fixed, the 429 wait is over, or
        // start() has managed to build a client.
        if !paused && !waiting_out_429 && self.client.is_some() {
            self.retry_after = None;
            // Clone the list so we can call `&mut self` methods inside the loop.
            for file in self.files.clone() {
                if !self.check_file(&file) {
                    break; // An error that stops this round; see check_file.
                }
            }
            if self.last_error_is_none() && !self.files.is_empty() {
                let n = self.files.len();
                let s = if n == 1 { "" } else { "s" };
                self.set_status(|st| st.activity = format!("Watching {n} account{s} for new murloc figures"));
            }
        }

        self.next_check = match self.retry_after {
            Some(t) if t > Instant::now() => t,
            _ => Instant::now() + self.backoff.unwrap_or(CHECK_EVERY),
        };
        let phase = if paused {
            Phase::Paused
        } else if self.retry_after.is_some() {
            Phase::RateLimited
        } else if self.backoff.is_some() {
            Phase::Retrying
        } else if self.client.is_none() || self.files.is_empty() {
            Phase::SetupNeeded
        } else {
            Phase::Watching
        };
        self.set_phase(phase);
    }

    /// Checks one account's file and uploads its export if it's new.
    /// Returns `false` when the rest of this round should be skipped
    /// (a bad token, a 429, or the website being unreachable).
    fn check_file(&mut self, file: &SavedVarFile) -> bool {
        // 1. Has the file changed since we last read it?
        let modified = match std::fs::metadata(&file.path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(e) => {
                let msg = format!("Couldn't look at {}: {e}", file.path.display());
                self.set_account(&file.account, AccountState::ReadError(msg.clone()));
                self.set_status(|s| s.last_error = Some(msg));
                return true;
            }
        };
        if self.last_seen.get(&file.path) == Some(&modified) {
            return true;
        }

        // 2. Read it and pull out murlocExport.
        let text = match std::fs::read_to_string(&file.path) {
            Ok(t) => t,
            Err(e) => {
                let msg = format!("Couldn't read {}: {e}", file.path.display());
                self.set_account(&file.account, AccountState::ReadError(msg.clone()));
                self.set_status(|s| s.last_error = Some(msg));
                return true;
            }
        };
        // Phase 2: the same file holds the group finder's outbox.
        if self.config.lfg_enabled {
            self.relay_signups(file, &text);
        }

        let export = match savedvars::read_key(&text, "murlocExport") {
            Ok(Some(export)) => export,
            Ok(None) => {
                // Older addon, or nothing to report yet. Not an error, but say so
                // once, because "nothing happens" is otherwise a mystery.
                self.last_seen.insert(file.path.clone(), modified);
                if self.set_account(&file.account, AccountState::NoExport) != Some(AccountState::NoExport) {
                    let why = match self.status.lock().ok().and_then(|s| s.addon.clone()?.too_old_for_murlocs) {
                        Some(min) => format!("update the addon to {min} or later"),
                        None => "this addon version doesn't send figures".to_string(),
                    };
                    self.event(Level::Warn, format!("{}: no murlocExport yet ({why})", file.account));
                }
                return true;
            }
            Err(e) => {
                // Most likely the game was halfway through writing the file.
                // Don't record the modified time, so the next tick tries again.
                let msg = format!("Couldn't read the addon's data for {}: {e}", file.account);
                self.set_account(&file.account, AccountState::ReadError(msg.clone()));
                self.set_status(|s| s.last_error = Some(msg));
                return true;
            }
        };

        // For the "This week" panel: keep the newest export's figures.
        self.note_report(&export);

        // 3. Already sent exactly this text?
        let hash = sha256_hex(&export);
        if self.config.export_hashes.get(&file.account) == Some(&hash) {
            self.last_seen.insert(file.path.clone(), modified);
            // Keep "Uploaded …" if it was sent this session; otherwise it's simply up to date.
            let current = self.account_state(&file.account);
            if !matches!(current, Some(AccountState::Uploaded { .. } | AccountState::Rejected(_))) {
                self.set_account(&file.account, AccountState::UpToDate);
            }
            return true;
        }

        // 4. Upload it.
        self.set_phase(Phase::Uploading);
        self.set_status(|s| s.activity = format!("Uploading murloc figures from {}…", file.account));
        let Some(client) = &self.client else { return false };
        match client.post_murlocs(&export) {
            Ok(result) => {
                self.remember_upload(file, hash, modified);
                self.backoff = None;
                let at = SystemTime::now();
                self.set_account(&file.account, AccountState::Uploaded { at, changed: result.changed });
                if !result.unmatched.is_empty() {
                    self.event(
                        Level::Warn,
                        format!("The website couldn't match: {}", result.unmatched.join(", ")),
                    );
                }
                let upload = Upload { at, account: file.account.clone(), result };
                self.set_status(|s| {
                    s.last_upload = Some(upload);
                    s.last_error = None;
                });
                true
            }
            Err(e @ ApiError::Rejected(_)) => {
                // Sending the same text again won't help, so remember it anyway.
                self.remember_upload(file, hash, modified);
                self.set_account(&file.account, AccountState::Rejected(e.to_string()));
                self.set_status(|s| s.last_error = Some(e.to_string()));
                true
            }
            Err(e @ (ApiError::Unauthorized | ApiError::NotOfficer)) => {
                self.set_status(|s| {
                    s.paused = true;
                    s.last_error = Some(e.to_string());
                });
                false
            }
            Err(e @ ApiError::RateLimited(wait)) => {
                self.retry_after = Some(Instant::now() + wait);
                self.set_status(|s| s.last_error = Some(e.to_string()));
                false
            }
            Err(e @ ApiError::Transient(_)) => {
                // Not remembered, so it's sent again next time. Wait longer each time.
                let next = self.backoff.map_or(CHECK_EVERY, |b| (b * 2).min(MAX_BACKOFF));
                self.backoff = Some(next);
                self.set_status(|s| s.last_error = Some(format!("{e} (trying again in {}s)", next.as_secs())));
                false
            }
            Err(e @ ApiError::NotAvailable) => {
                // 404: the site has no /murlocs. Nothing to retry until it does.
                self.last_seen.insert(file.path.clone(), modified);
                self.set_status(|s| s.last_error = Some(format!("Uploading murloc figures: {e}")));
                false
            }
        }
    }

    fn account_state(&self, account: &str) -> Option<AccountState> {
        let s = self.status.lock().ok()?;
        s.accounts.iter().find(|a| a.name == account).map(|a| a.state.clone())
    }

    /// Records that this account's export has been dealt with: the hash goes in
    /// config.toml (so a restart doesn't resend it) and the modified time in memory.
    fn remember_upload(&mut self, file: &SavedVarFile, hash: String, modified: SystemTime) {
        self.config.export_hashes.insert(file.account.clone(), hash);
        self.save_config();
        self.last_seen.insert(file.path.clone(), modified);
    }

    fn last_error_is_none(&self) -> bool {
        self.status.lock().map(|s| s.last_error.is_none()).unwrap_or(true)
    }

    /// Reads the installed addon's version and asks the website for the latest
    /// (step 13), then shows the result in the header.
    fn check_addon(&self, game_dir: &std::path::Path) {
        let installed = game::installed_addon_version(game_dir).and_then(|v| Version::parse(&v));
        let info = match self.client.as_ref().map(|c| c.addon_info()) {
            Some(Ok(info)) => Some(info),
            Some(Err(ApiError::NotAvailable)) | None => None, // the website doesn't publish it (yet)
            Some(Err(e)) => {
                self.event(Level::Info, format!("Couldn't check the addon version: {e}"));
                None
            }
        };
        let view = addon::addon_status(installed, info.as_ref());
        if let Some(min) = view.too_old_for_murlocs {
            self.event(Level::Warn, format!("The guild addon is too old to send murloc figures: update it to {min} or later"));
        } else if view.update_available {
            if let Some(latest) = view.latest {
                self.event(Level::Info, format!("Guild addon {latest} is out"));
            }
        }
        self.set_status(|s| s.addon = Some(view));
    }

    /// Parses an export for the "This week" panel (step 11), keeping the newest
    /// one across accounts. A parse failure is only shown in the panel: the
    /// upload goes ahead regardless, since the website does the real reading.
    fn note_report(&self, export: &str) {
        match report::parse_export(export) {
            Ok(r) => self.set_status(|s| {
                if s.report.as_ref().is_none_or(|old| r.exported_at >= old.exported_at) {
                    s.report = Some(r);
                    s.report_note = None;
                }
            }),
            Err(e) => self.set_status(|s| {
                if s.report.is_none() {
                    s.report_note = Some(e);
                }
            }),
        }
    }

    /// Copies the group finder state into `Status` for the dashboard (finished
    /// boilerplate). Call it after changing `self.lfg`.
    fn publish_lfg(&self) {
        let view = self.config.lfg_enabled.then(|| {
            let mut upcoming: Vec<Run> = self
                .lfg
                .snapshot
                .iter()
                .flat_map(|s| s.runs.iter())
                .filter_map(Run::from_value)
                .collect();
            upcoming.sort_by_key(|r| r.starts_at);
            LfgView {
                runs_known: self.lfg.snapshot.as_ref().map_or(0, |s| s.runs.len()),
                upcoming: upcoming.into_iter().take(3).collect(),
                signups_sent: self.lfg.signups_sent,
                pending: self.lfg.pending.len(),
                last_poll: self.lfg.last_poll,
            }
        });
        self.set_status(|s| s.lfg = view);
    }

    // -----------------------------------------------------------------------
    // Phase 2: the group finder. Yours: GUIDE.md step 10.
    // Each one only reports "not written yet" until you fill it in, so turning
    // `lfg_enabled` on before then is safe.
    // -----------------------------------------------------------------------

    /// Runs from `start()` when `lfg_enabled` is on (after the game folder is known).
    ///
    /// 1. `data_addon::read_acks(game_dir)` → `self.lfg.acks`. If Data.lua is
    ///    missing, also forget `config.lfg_etag` (and save config), so the first
    ///    poll gets a full 200 instead of a 304 with nothing to write.
    /// 2. `data_addon::interface_version(game_dir)`, falling back to
    ///    `data_addon::FALLBACK_INTERFACE` → `self.lfg.interface`.
    fn lfg_start(&mut self) {
        let _ = (&self.lfg.acks, &self.lfg.interface, data_addon::FALLBACK_INTERFACE);
        self.event(Level::Info, "Group finder: step 10 (Worker::lfg_start) isn't written yet");
    }

    /// Runs when `self.lfg.next_poll` is due (every `LFG_POLL_EVERY`).
    ///
    /// 1. No client, or paused? Schedule the next poll and return.
    /// 2. `client.get_lfg(config.lfg_etag)`:
    ///      NotModified → note `last_poll`, nothing else
    ///      Fresh       → check `version == 1`; store the snapshot and save the
    ///                    new ETag in config; prune acks; write the files (step 3)
    ///      NotAvailable (404) → say once that the site has no group finder yet
    ///      RateLimited → next poll after the wait (and at least `LFG_MIN_GAP`)
    ///      Transient   → show it; try again next poll
    ///      Unauthorized / NotOfficer → paused, as in `check_file`
    /// 3. Writing: `data_addon::write_if_changed` for the .toc (`render_toc`) and
    ///    Data.lua (`render_data`). Only log an event when it actually wrote.
    /// 4. `self.lfg.next_poll = Some(now + LFG_POLL_EVERY)`, then `self.publish_lfg()`.
    fn poll_lfg(&mut self) {
        self.lfg.next_poll = Some(Instant::now() + LFG_POLL_EVERY);
        self.event(Level::Info, "Group finder: step 10 (Worker::poll_lfg) isn't written yet");
    }

    /// Runs from `check_file` each time an account's saved data changed, with the
    /// file's text.
    ///
    /// 1. `savedvars::read_key(text, "lfgOutbox")`. `None` → nothing to send
    ///    (an older addon, or an empty outbox); `Err` → show it.
    /// 2. `lfg::parse_outbox`, then `lfg::pending(&outbox, &acks)`. Add those to
    ///    `self.lfg.pending` (skip client_ids already there).
    /// 3. Nothing pending → return. Otherwise `client.post_signups(&pending)`:
    ///      Ok(results) → an `Ack::from_result` per result into `acks`; remove those
    ///                    from `pending`; count them in `signups_sent`; log rejections
    ///                    with their reason
    ///      Err(...)    → keep `pending` (it's resent next time; the site ignores
    ///                    repeats). React to the error kind as in `poll_lfg`
    /// 4. Prune acks, rewrite Data.lua (snapshot + acks), `self.publish_lfg()`.
    fn relay_signups(&mut self, file: &SavedVarFile, text: &str) {
        let _ = (text, &self.lfg.pending, lfg::ACK_KEEP_SECS);
        self.event(
            Level::Info,
            format!("Group finder: step 10 (Worker::relay_signups) isn't written yet ({})", file.account),
        );
    }
}

/// SHA-256 of the text, as 64 lowercase hex characters.
fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}
