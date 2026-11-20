//! The background thread that does the actual work.
//!
//! The dashboard (main thread) never waits on the network or the disk. It sends
//! `Command`s here and redraws from the shared `Status` a few times a second.
//!
//! The worker is also the only thing that saves config.toml, so the dashboard and
//! the worker can never overwrite each other's changes (the token versus the export hashes).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::addon::{self, AddonView};
use crate::api::{self, ApiError, ImportResult, LfgFetch};
use crate::config::Config;
use crate::eventlog::{self, Event, Level};
use crate::game::{self, SavedVarFile};
use crate::lfg::{self, Ack, Acks, LfgSnapshot, Run};
use crate::report::{self, Report};
use crate::update::UpdateInfo;
use crate::version::Version;
use crate::data_addon;
use crate::savedvars::SavedVars;

/// How often to look at the saved-data files' modified times (the spec says 30 seconds).
pub const CHECK_EVERY: Duration = Duration::from_secs(30);
/// Backoff ceiling for 5xx and network errors (the spec says 15 minutes).
pub const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// Phase 2: how often to ask the website for runs (the spec's default: 5 minutes).
pub const LFG_POLL_EVERY: Duration = Duration::from_secs(5 * 60);
/// Phase 2: never poll /lfg more often than this (the spec: at most once a minute).
pub const LFG_MIN_GAP: Duration = Duration::from_secs(60);
/// Phase 2: a run stays on the dashboard this long after it starts.
pub const RUN_SHOWN_AFTER_START: Duration = Duration::from_secs(3 * 60 * 60);

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

/// How one website feature last answered, for the dashboard.
#[derive(Debug, Default, Clone, PartialEq)]
pub enum Service {
    /// Not asked yet (no token, or still starting).
    #[default]
    Unknown,
    /// It answered (a 422 counts: the website was there to say no).
    Ok,
    /// 404: the website doesn't have this feature yet.
    NotOnSite,
    /// 5xx, network down or timed out. The details are in the Problem line and events.
    Unreachable,
    /// 429: not asking again before this moment.
    RateLimited(Instant),
    /// 401/403: the token was refused.
    Refused,
}

impl Service {
    /// The state an error leaves a feature in.
    pub fn from_error(e: &ApiError) -> Service {
        match e {
            ApiError::Rejected(_) => Service::Ok,
            ApiError::NotAvailable => Service::NotOnSite,
            ApiError::Transient(_) => Service::Unreachable,
            ApiError::RateLimited(wait) => Service::RateLimited(Instant::now() + *wait),
            ApiError::Unauthorized | ApiError::NotOfficer => Service::Refused,
        }
    }

    fn from_result<T>(r: &Result<T, ApiError>) -> Service {
        r.as_ref().map_or_else(Service::from_error, |_| Service::Ok)
    }
}

/// The website's health, and each feature's state. The group finder's state
/// lives in `LfgView`.
#[derive(Debug, Default, Clone)]
pub struct Website {
    /// "murlocdefenseleague.org", or "127.0.0.1:8787" when pointed at the mock.
    pub host: String,
    /// When the website last gave a proper answer, and how long that took.
    pub last_answer: Option<SystemTime>,
    pub last_latency: Option<Duration>,
    /// Whether the latest request got a proper answer. `None` before the first.
    pub reachable: Option<bool>,
    /// `GET /me` and `POST /murlocs`.
    pub uploads: Service,
    /// `GET /addon`.
    pub addon_versions: Service,
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
    /// The installed addon's version, and what the website says.
    pub addon: Option<AddonView>,
    /// How each website feature last answered.
    pub website: Website,
    /// The newest Impact Report export read from any account.
    pub report: Option<Report>,
    /// Why there's no report to show (e.g. the export didn't parse).
    pub report_note: Option<String>,
}

/// Phase 2: what the dashboard's Group finder panel shows. Built by `Worker::publish_lfg`.
#[derive(Debug, Default, Clone)]
pub struct LfgView {
    /// How many runs the last `GET /lfg` returned.
    pub runs_known: usize,
    /// Runs that haven't finished, soonest first. The dashboard shows as many as fit.
    pub upcoming: Vec<Run>,
    /// Sign-ups the website has answered this session.
    pub signups_sent: usize,
    /// Sign-ups read from an outbox but not answered yet (they're retried).
    pub pending: usize,
    /// When `GET /lfg` last answered (200 or 304).
    pub last_poll: Option<SystemTime>,
    /// How `GET /lfg` / `POST /lfg/signups` last answered.
    pub service: Service,
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
                game_dir: None,
                signed_in: false,
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
    told_not_available: bool,
    service: Service,
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
    /// The game folder `start()` settled on (configured or detected).
    game_dir: Option<PathBuf>,
    /// `GET /me` has accepted the token. False while the website couldn't be
    /// reached at start-up: `tick` asks again until it answers.
    signed_in: bool,
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
                    self.tick(true);
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
                        self.tick(false);
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
        if s.last_error != before.1
            && let Some(e) = s.last_error.clone()
        {
            s.push_event(Event::now(Level::Error, e));
        }
        if let Some(u) = s.last_upload.clone()
            && Some(u.at) != before.2
        {
            let what = if u.result.changed { "new figures" } else { "no change (the website already had them)" };
            s.push_event(Event::now(Level::Ok, format!("Uploaded {}: {what}", u.account)));
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

    /// After a 401/403 (or a token whose owner isn't an officer): stop calling
    /// the website until the settings change.
    fn pause(&self, why: &ApiError) {
        self.set_status(|s| {
            s.paused = true;
            s.last_error = Some(why.to_string());
        });
        self.set_phase(Phase::Paused);
    }

    /// `GET /me`. Sets `signed_in` when the token checks out, and pauses when
    /// it's refused. Returns false if paused. A website that couldn't answer
    /// isn't the token's fault: that's shown, and `tick` asks again later.
    fn sign_in(&mut self) -> bool {
        let Some(client) = &self.client else { return false };
        let result = client.me();
        let service = Service::from_result(&result);
        self.set_status(|s| s.website.uploads = service);
        self.publish_contact();
        match result {
            Ok(me) if me.officer => {
                self.event(Level::Ok, format!("Signed in as {}", me.name));
                self.set_status(|s| {
                    s.signed_in_as = Some(me.name);
                    s.last_error = None;
                });
                self.signed_in = true;
            }
            Ok(_) => self.pause(&ApiError::NotOfficer),
            Err(e @ (ApiError::Unauthorized | ApiError::NotOfficer)) => self.pause(&e),
            Err(e) => self.set_status(|s| s.last_error = Some(format!("Checking the token: {e}"))),
        }
        !self.is_paused()
    }

    /// Copies how the latest request went into the website health line. Call
    /// it after each API call.
    fn publish_contact(&self) {
        let Some(contact) = self.client.as_ref().and_then(api::Client::last_contact) else { return };
        self.set_status(|s| {
            s.website.reachable = Some(contact.answered);
            if contact.answered {
                s.website.last_answer = Some(contact.at);
                s.website.last_latency = Some(contact.took);
            }
        });
    }

    fn is_paused(&self) -> bool {
        self.status.lock().map(|s| s.paused).unwrap_or(false)
    }

    /// Phase 2: the website answered 404 for the group finder. Said once per start.
    fn lfg_not_available(&mut self) {
        if !self.lfg.told_not_available {
            self.event(Level::Info, "Group finder: the website doesn't have it yet");
            self.lfg.told_not_available = true;
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
        self.signed_in = false;
        self.retry_after = None;
        self.backoff = None;
        self.files.clear();
        self.last_seen.clear();
        self.game_dir = None;
        self.lfg = LfgState::default();
        self.set_status(|s| {
            s.paused = false;
            s.signed_in_as = None;
            s.last_error = None;
            s.game_dir = None;
            s.accounts.clear();
            s.addon = None;
            s.website = Website { host: host_of(&self.config.api_base), ..Website::default() };
        });
        self.publish_lfg();
        self.next_check = Instant::now() + CHECK_EVERY;

        if self.config.token.is_empty() {
            self.set_phase(Phase::SetupNeeded);
            self.set_status(|s| s.activity = "No token yet: press t and paste one from the website".to_string());
            return;
        }

        self.set_phase(Phase::CheckingToken);
        self.client = Some(api::Client::new(&self.config.api_base, &self.config.token));
        if !self.sign_in() {
            return;
        }

        let Some(dir) = self.config.game_dir.clone().or_else(game::detect) else {
            self.set_phase(Phase::SetupNeeded);
            self.set_status(|s| s.activity = "No game folder: press d to detect it, or f to type it".to_string());
            return;
        };
        self.game_dir = Some(dir.clone());

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

        if self.config.lfg_enabled {
            self.lfg_start();
            self.lfg.next_poll = Some(Instant::now());
        }
        self.publish_lfg();
    }

    /// Runs every `CHECK_EVERY` (and when `u` is pressed).
    ///
    /// 1. Paused, or still inside a 429 wait? Just schedule the next check.
    ///    (If the website was down when `start` checked the token, ask again first.)
    /// 2. For each file in `self.files`: compare its modified time with `last_seen`.
    ///    Unchanged → skip it.
    /// 3. Read it, run it (`SavedVars::parse`), then `get("murlocExport")`. `None` → skip it.
    /// 4. SHA-256 the text. Same as `config.export_hashes[account]` → skip it.
    /// 5. `post_murlocs`, then:
    ///    * Ok          → save the hash, record the upload, clear the error, reset backoff
    ///    * Rejected    → save the hash anyway, show the message
    ///    * Unauthorized / NotOfficer → paused
    ///    * RateLimited → set retry_after
    ///    * Transient   → DON'T save the hash or last_seen; grow the backoff (×2, max 15 min)
    /// 6. Set `next_check`: now + backoff if backing off, else now + CHECK_EVERY.
    ///
    /// `asked` is true when the officer pressed `u`, so quiet outcomes get a log line.
    fn tick(&mut self, asked: bool) {
        let waiting_out_429 = self.retry_after.is_some_and(|t| Instant::now() < t);
        // The website was down when start() checked the token: ask again first.
        if !waiting_out_429 && self.client.is_some() && !self.signed_in && !self.is_paused() {
            self.sign_in();
        }
        let paused = self.is_paused();

        // Nothing to do until the token is fixed, the 429 wait is over, or
        // start() has managed to build a client.
        if asked {
            let why = if paused {
                Some("paused until the token is fixed".to_string())
            } else if waiting_out_429 {
                Some("waiting out the website's rate limit".to_string())
            } else if self.client.is_none() {
                Some("no token set yet".to_string())
            } else if self.files.is_empty() {
                Some("no accounts found in the game folder".to_string())
            } else {
                None
            };
            if let Some(why) = why {
                self.event(Level::Warn, format!("Didn't check: {why}"));
            }
        }
        if !paused && !waiting_out_429 && self.client.is_some() {
            self.retry_after = None;
            // Clone the list so we can call `&mut self` methods inside the loop.
            for file in self.files.clone() {
                if !self.check_file(&file, asked) {
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
    fn check_file(&mut self, file: &SavedVarFile, asked: bool) -> bool {
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
        let saved = match SavedVars::parse(&text) {
            Ok(saved) => saved,
            Err(e) => {
                // Most likely the game was halfway through writing the file.
                // Don't record the modified time, so the next tick tries again.
                let msg = format!("Couldn't read the addon's data for {}: {e}", file.account);
                self.set_account(&file.account, AccountState::ReadError(msg.clone()));
                self.set_status(|s| s.last_error = Some(msg));
                return true;
            }
        };

        // Phase 2: the same file holds the group finder's outbox.
        if self.config.lfg_enabled {
            self.relay_signups(file, &saved);
        }

        let export = match saved.get("murlocExport") {
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
                let msg = format!("Couldn't read the murloc figures for {}: {e}", file.account);
                self.last_seen.insert(file.path.clone(), modified);
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
            if asked {
                self.event(Level::Ok, format!("{}: up to date (no new figures since the last upload)", file.account));
            }
            return true;
        }

        // 4. Upload it.
        self.set_phase(Phase::Uploading);
        // This week's kills across all characters, for the log line. Left out if
        // the export doesn't parse or has no W lines.
        let this_week = report::parse_export(&export).ok().and_then(|r| {
            let week = report::latest_week(&r)?;
            Some(r.weeks.iter().filter(|w| w.week_start == week).map(|w| w.kills).sum::<u32>())
        });
        let kills = this_week.map(|k| format!(" ({k} kills this week)")).unwrap_or_default();
        self.set_status(|s| s.activity = format!("Uploading murloc figures from {}{kills}…", file.account));
        let Some(client) = &self.client else { return false };
        let result = client.post_murlocs(&export);
        let service = Service::from_result(&result);
        self.set_status(|s| s.website.uploads = service);
        self.publish_contact();
        match result {
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
                self.pause(&e);
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

    /// Reads the installed addon's version and asks the website for the latest,
    /// then shows the result in the header.
    fn check_addon(&self, game_dir: &Path) {
        let installed = game::installed_addon_version(game_dir).and_then(|v| Version::parse(&v));
        let result = self.client.as_ref().map(|c| c.addon_info());
        let service = result.as_ref().map_or(Service::Unknown, Service::from_result);
        self.set_status(|s| s.website.addon_versions = service);
        self.publish_contact();
        let info = match result {
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
        } else if view.update_available
            && let Some(latest) = view.latest
        {
            self.event(Level::Info, format!("Guild addon {latest} is out"));
        }
        self.set_status(|s| s.addon = Some(view));
    }

    /// Parses an export for the "This week" panel, keeping the newest
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
                // Logged once per different error; the panel just says it couldn't read them.
                if s.report.is_none() && s.report_note.as_ref() != Some(&e) {
                    s.push_event(Event::now(Level::Warn, format!("Couldn't read this week's figures: {e}")));
                    s.report_note = Some(e);
                }
            }),
        }
    }

    /// Copies the group finder state into `Status` for the dashboard (finished
    /// boilerplate). Call it after changing `self.lfg`.
    fn publish_lfg(&self) {
        let view = self.config.lfg_enabled.then(|| {
            // Runs that started more than a few hours ago are over; leave them out.
            let cutoff = jiff::Timestamp::now().as_second() - RUN_SHOWN_AFTER_START.as_secs() as i64;
            let mut upcoming: Vec<Run> = self
                .lfg
                .snapshot
                .iter()
                .flat_map(|s| s.runs.iter())
                .filter_map(Run::from_value)
                .filter(|r| r.starts_at >= cutoff)
                .collect();
            upcoming.sort_by_key(|r| r.starts_at);
            LfgView {
                runs_known: self.lfg.snapshot.as_ref().map_or(0, |s| s.runs.len()),
                upcoming,
                signups_sent: self.lfg.signups_sent,
                pending: self.lfg.pending.len(),
                last_poll: self.lfg.last_poll,
                service: self.lfg.service.clone(),
            }
        });
        self.set_status(|s| s.lfg = view);
    }

    // -----------------------------------------------------------------------
    // Phase 2: the group finder.
    // -----------------------------------------------------------------------

    /// Runs from `start()` when `lfg_enabled` is on (after the game folder is known).
    ///
    /// 1. `data_addon::read_acks(game_dir)` → `self.lfg.acks`. If Data.lua is
    ///    missing, also forget `config.lfg_etag` (and save config), so the first
    ///    poll gets a full 200 instead of a 304 with nothing to write.
    /// 2. `data_addon::interface_version(game_dir)`, falling back to
    ///    `data_addon::FALLBACK_INTERFACE` → `self.lfg.interface`.
    fn lfg_start(&mut self) {
        let Some(dir) = self.game_dir.clone() else { return };

        // 1. Acks from our own Data.lua.
        if !data_addon::data_path(&dir).exists() {
            // No Data.lua: forget the ETag so the first poll gets a full answer.
            self.config.lfg_etag = None;
            self.save_config();
        }
        self.lfg.acks = match data_addon::read_acks(&dir) {
            Ok(acks) => acks,
            Err(e) => {
                self.event(Level::Warn, format!("Group finder: couldn't read Data.lua ({e}); starting with no acks"));
                Acks::new()
            }
        };

        // 2. The Interface number for our .toc.
        self.lfg.interface = data_addon::interface_version(&dir)
            .unwrap_or_else(|| data_addon::FALLBACK_INTERFACE.to_string());
    }

    /// Runs when `self.lfg.next_poll` is due (every `LFG_POLL_EVERY`).
    ///
    /// 1. Not signed in, or paused? Schedule the next poll and return.
    /// 2. `client.get_lfg(config.lfg_etag)`:
    ///    * NotModified → note `last_poll`, nothing else
    ///    * Fresh → check `version == 1`; store the snapshot and save the new
    ///      ETag in config; prune acks; write the files (step 3)
    ///    * NotAvailable (404) → say once that the site has no group finder yet
    ///    * RateLimited → next poll after the wait (and at least `LFG_MIN_GAP`)
    ///    * Transient → show it; try again next poll
    ///    * Unauthorized / NotOfficer → paused, as in `check_file`
    /// 3. Writing: `data_addon::write_if_changed` for the .toc (`render_toc`) and
    ///    Data.lua (`render_data`). Only log an event when it actually wrote.
    /// 4. `self.lfg.next_poll = Some(now + LFG_POLL_EVERY)`, then `self.publish_lfg()`.
    fn poll_lfg(&mut self) {
        let mut next = LFG_POLL_EVERY;
        let client = self.client.as_ref().filter(|_| self.signed_in && !self.is_paused());
        let Some(client) = client else {
            self.lfg.next_poll = Some(Instant::now() + next);
            return;
        };

        let result = client.get_lfg(self.config.lfg_etag.as_deref());
        self.lfg.service = Service::from_result(&result);
        self.publish_contact();
        match result {
            Ok(LfgFetch::NotModified) => {
                self.lfg.last_poll = Some(SystemTime::now());
            }
            Ok(LfgFetch::Fresh { snapshot, etag }) => {
                self.lfg.last_poll = Some(SystemTime::now());
                if snapshot.version == 1 {
                    self.lfg.snapshot = Some(snapshot);
                    self.config.lfg_etag = etag;
                    self.save_config();
                    lfg::prune_acks(&mut self.lfg.acks, jiff::Timestamp::now().as_second());
                    self.write_data_files();
                } else {
                    self.set_status(|s| {
                        s.last_error =
                            Some("The website's group finder speaks a newer version: update the app".to_string())
                    });
                }
            }
            Err(ApiError::NotAvailable) => self.lfg_not_available(),
            Err(ApiError::RateLimited(wait)) => next = wait.max(LFG_MIN_GAP),
            Err(e @ (ApiError::Unauthorized | ApiError::NotOfficer)) => self.pause(&e),
            Err(e) => self.event(Level::Warn, format!("Group finder: {e}")),
        }

        self.lfg.next_poll = Some(Instant::now() + next);
        self.publish_lfg();
    }

    /// Writes the data addon's .toc and Data.lua from the current state.
    /// Each is only rewritten when its text changed.
    fn write_data_files(&mut self) {
        let Some(dir) = self.game_dir.clone() else { return };
        let toc = data_addon::render_toc(&self.lfg.interface);
        let data = data_addon::render_data(self.lfg.snapshot.as_ref(), &self.lfg.acks);
        for (path, text) in [(data_addon::toc_path(&dir), toc), (data_addon::data_path(&dir), data)] {
            match data_addon::write_if_changed(&path, &text) {
                Ok(true) => self.event(Level::Info, format!("Group finder: wrote {}", path.display())),
                Ok(false) => {} // unchanged
                Err(e) => self.event(Level::Warn, format!("Group finder: couldn't write {}: {e}", path.display())),
            }
        }
    }

    /// Runs from `check_file` each time an account's saved data changed.
    ///
    /// 1. `saved.get("lfgOutbox")`. `None` → nothing to send
    ///    (an older addon, or an empty outbox); `Err` → show it.
    /// 2. `lfg::parse_outbox`, then `lfg::pending(&outbox, &acks)`. Add those to
    ///    `self.lfg.pending` (skip client_ids already there).
    /// 3. Nothing pending → return. Otherwise `client.post_signups(&pending)`:
    ///    * Ok(results) → an `Ack::from_result` per result into `acks`; remove those
    ///      from `pending`; count them in `signups_sent`; log rejections with their reason
    ///    * Err(...) → keep `pending` (it's resent next time; the site ignores
    ///      repeats). React to the error kind as in `poll_lfg`
    /// 4. Prune acks, rewrite Data.lua (snapshot + acks), `self.publish_lfg()`.
    fn relay_signups(&mut self, file: &SavedVarFile, saved: &SavedVars) {
        // 1. The outbox, if this account has one.
        let json = match saved.get("lfgOutbox") {
            Ok(Some(json)) => json,
            Ok(None) => return,
            Err(e) => {
                self.event(Level::Warn, format!("Group finder: couldn't read {}'s outbox: {e}", file.account));
                return;
            }
        };

        // 2. What still needs sending, added to anything left over from last time.
        let outbox = match lfg::parse_outbox(&json) {
            Ok(outbox) => outbox,
            Err(e) => {
                self.event(Level::Warn, format!("Group finder: {}'s outbox isn't valid: {e}", file.account));
                return;
            }
        };
        for item in lfg::pending(&outbox, &self.lfg.acks) {
            let id = lfg::client_id(&item);
            if !self.lfg.pending.iter().any(|p| lfg::client_id(p) == id) {
                self.lfg.pending.push(item);
            }
        }

        // 3. Send them.
        if self.lfg.pending.is_empty() {
            return;
        }
        // Kept pending until the token is confirmed; sent with the next change.
        let client = self.client.as_ref().filter(|_| self.signed_in && !self.is_paused());
        let Some(client) = client else { return };
        let result = client.post_signups(&self.lfg.pending);
        self.lfg.service = Service::from_result(&result);
        self.publish_contact();
        match result {
            Ok(results) => {
                let now = jiff::Timestamp::now().as_second();
                for r in &results {
                    self.lfg.acks.insert(r.client_id.clone(), Ack::from_result(r, now));
                    if r.status != "ok" {
                        let reason = r.reason.as_deref().unwrap_or("no reason given");
                        self.event(Level::Warn, format!("Group finder: sign-up {} rejected ({reason})", r.client_id));
                    }
                }
                self.lfg.signups_sent += results.len();
                // Answered items are done; anything the site didn't answer stays pending.
                let acks = &self.lfg.acks;
                self.lfg.pending.retain(|p| lfg::client_id(p).is_none_or(|id| !acks.contains_key(id)));
                let n = results.len();
                let s = if n == 1 { "" } else { "s" };
                self.event(Level::Ok, format!("Group finder: sent {n} sign-up{s} from {}", file.account));
            }
            // Pending is kept either way: it's resent next time (the site ignores repeats).
            Err(e @ (ApiError::Unauthorized | ApiError::NotOfficer)) => self.pause(&e),
            Err(ApiError::NotAvailable) => self.lfg_not_available(),
            Err(e) => self.event(Level::Warn, format!("Group finder: sending sign-ups: {e}")),
        }

        // 4. Save the acks where the addon will see them.
        lfg::prune_acks(&mut self.lfg.acks, jiff::Timestamp::now().as_second());
        self.write_data_files();
        self.publish_lfg();
    }
}

/// The host (and port, if any) of the API's base URL, for the dashboard.
/// The whole string if it isn't a URL.
fn host_of(api_base: &str) -> String {
    let Ok(url) = reqwest::Url::parse(api_base) else { return api_base.to_string() };
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        (None, _) => api_base.to_string(),
    }
}

/// SHA-256 of some bytes (or text), as 64 lowercase hex characters.
pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data.as_ref()).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_for_the_dashboard() {
        assert_eq!(host_of("https://murlocdefenseleague.org/api/v1"), "murlocdefenseleague.org");
        assert_eq!(host_of("http://127.0.0.1:8787/api/v1"), "127.0.0.1:8787");
        assert_eq!(host_of("not a url"), "not a url");
    }

    #[test]
    fn errors_set_feature_state() {
        assert_eq!(Service::from_error(&ApiError::Rejected("no".into())), Service::Ok);
        assert_eq!(Service::from_error(&ApiError::NotAvailable), Service::NotOnSite);
        assert_eq!(Service::from_error(&ApiError::Transient("x".into())), Service::Unreachable);
        assert_eq!(Service::from_error(&ApiError::Unauthorized), Service::Refused);
    }
}
