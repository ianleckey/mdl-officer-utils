//! The full-screen dashboard. It reads keys, sends `Command`s to the worker, and
//! redraws from `Status` four times a second. It does no work itself, so a slow
//! network never freezes the screen.
//!
//! Keys: u upload now · t token · f folder · d detect folder · ↑↓ PgUp PgDn scroll
//! the events · q (or Ctrl-C) quit.

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use jiff::Timestamp;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::SetTitle;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::addon::AddonView;
use crate::art;
use crate::config::Config;
use crate::eventlog::{self, Level};
use crate::game;
use crate::lfg::Run;
use crate::{report, schedule};
use crate::worker::{AccountState, Command, LfgView, Phase, Status, Upload};

/// What the keyboard is doing right now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Normal,
    /// Typing a token. Shown as dots, never as text.
    EditToken,
    /// Typing a game folder path.
    EditFolder,
}

/// The dashboard's own state (everything else comes from `Status`).
struct Ui {
    tx: Sender<Command>,
    // Our copies of the settings. Changing one sends both to the worker, which
    // saves them and starts over.
    token: String,
    game_dir: Option<PathBuf>,
    mode: Mode,
    input: String,
    /// How many events up from the newest the Events panel is scrolled.
    scroll: usize,
    /// A short message from a key press, shown in the footer for a few seconds.
    notice: Option<(String, Instant)>,
    log_path: Option<PathBuf>,
    /// The guild crest fills the screen until then (any key skips it).
    splash_until: Option<Instant>,
    quit: bool,
}

/// How long the start-up splash stays up.
const SPLASH_FOR: Duration = Duration::from_millis(1500);
/// The header shows the small crest on terminals at least this tall.
const CREST_MIN_ROWS: u16 = 40;

/// Runs the dashboard until the officer quits. Always puts the terminal back.
pub fn run(config: Config, status: Arc<Mutex<Status>>, tx: Sender<Command>) -> io::Result<()> {
    let mut terminal = ratatui::init(); // Also restores the terminal if we panic.
    let _ = execute!(io::stdout(), EnableBracketedPaste, SetTitle("Murloc Defence League"));

    let mut ui = Ui {
        tx: tx.clone(),
        token: config.token,
        game_dir: config.game_dir,
        mode: Mode::Normal,
        input: String::new(),
        scroll: 0,
        notice: None,
        log_path: eventlog::log_path(),
        splash_until: Some(Instant::now() + SPLASH_FOR),
        quit: false,
    };
    let result = ui.event_loop(&mut terminal, &status);

    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    let _ = tx.send(Command::Quit);
    result
}

impl Ui {
    fn event_loop(&mut self, terminal: &mut DefaultTerminal, status: &Arc<Mutex<Status>>) -> io::Result<()> {
        while !self.quit {
            // Take a copy so the worker isn't kept waiting on the lock while we draw.
            let snapshot = status.lock().map(|s| s.clone()).unwrap_or_default();
            if self.splash_until.is_some_and(|t| Instant::now() >= t) {
                self.splash_until = None;
            }
            terminal.draw(|frame| {
                if self.splash_until.is_some() {
                    draw_splash(frame)
                } else {
                    draw(frame, &snapshot, self)
                }
            })?;

            let tick = if self.splash_until.is_some() { 50 } else { 250 };
            if event::poll(Duration::from_millis(tick))? {
                let ev = event::read()?;
                if self.splash_until.is_some() {
                    // Any key (or click, or resize) skips the splash; don't act on it.
                    if matches!(ev, TermEvent::Key(_)) {
                        self.splash_until = None;
                    }
                    continue;
                }
                match ev {
                    // Windows also reports key releases; only act on presses.
                    TermEvent::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key, &snapshot),
                    TermEvent::Paste(text) if self.mode != Mode::Normal => {
                        self.input.push_str(text.trim_end_matches(['\r', '\n']));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn on_key(&mut self, key: KeyEvent, status: &Status) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::Normal => self.on_normal_key(key.code, status),
            Mode::EditToken | Mode::EditFolder => self.on_edit_key(key),
        }
    }

    fn on_normal_key(&mut self, code: KeyCode, status: &Status) {
        match code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('u') => {
                let _ = self.tx.send(Command::UploadNow);
                self.note("Checking the game's files now…");
            }
            KeyCode::Char('t') => {
                self.mode = Mode::EditToken;
                self.input.clear();
            }
            KeyCode::Char('f') => {
                self.mode = Mode::EditFolder;
                self.input = self.game_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
            }
            KeyCode::Char('d') => match game::detect() {
                Some(p) => {
                    self.note(&format!("Found {}", p.display()));
                    self.game_dir = Some(p);
                    self.send_settings();
                }
                None => self.note("Couldn't find the game folder. Press f to type it in."),
            },
            KeyCode::Up => self.scroll_by(1, status),
            KeyCode::Down => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageUp => self.scroll_by(10, status),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Home => self.scroll = status.events.len(),
            KeyCode::End => self.scroll = 0,
            _ => {}
        }
    }

    fn scroll_by(&mut self, n: usize, status: &Status) {
        self.scroll = (self.scroll + n).min(status.events.len().saturating_sub(1));
    }

    fn on_edit_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.input.clear();
                self.note("Cancelled.");
            }
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => self.input.clear(),
            KeyCode::Char(c) => self.input.push(c),
            _ => {}
        }
    }

    fn submit(&mut self) {
        let text = self.input.trim().to_string();
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        self.input.clear();
        if text.is_empty() {
            self.note("Nothing typed; left as it was.");
            return;
        }
        match mode {
            Mode::EditToken => {
                self.token = text;
                self.send_settings();
                self.note("Token saved. Checking it with the website…");
            }
            Mode::EditFolder => {
                // Shells hand paths over with quotes sometimes; drop them.
                let p = PathBuf::from(text.trim_matches(|c| c == '"' || c == '\''));
                if game::looks_like_game_dir(&p) {
                    self.note("Folder saved.");
                } else {
                    self.note("Saved, but there's no WTF or Interface/AddOns in there. Is it the _forever_ folder?");
                }
                self.game_dir = Some(p);
                self.send_settings();
            }
            Mode::Normal => {}
        }
    }

    fn send_settings(&self) {
        let _ = self.tx.send(Command::SettingsChanged { token: self.token.clone(), game_dir: self.game_dir.clone() });
    }

    fn note(&mut self, text: &str) {
        self.notice = Some((text.to_string(), Instant::now()));
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

const LABEL: Style = Style::new().fg(Color::DarkGray);
const GOLD: Color = Color::Rgb(0xE6, 0xC2, 0x6E);

fn draw(frame: &mut Frame, s: &Status, ui: &Ui) {
    let tall = frame.area().height >= CREST_MIN_ROWS;
    let accounts_height = s.accounts.len().max(1) as u16 + 2;
    let mut header_height = 4 + u16::from(s.addon.is_some()) + u16::from(s.last_error.is_some());
    if tall {
        header_height = header_height.max(art::CREST_SMALL.len() as u16 + 2);
    }
    let lfg_height = s.lfg.as_ref().map_or(0, |l| l.upcoming.len().max(1) as u16 + 3);
    let week = week_lines(s);
    let week_height = week.len() as u16 + 2;
    let [header, accounts, upload, week_area, lfg, events, footer] = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Length(accounts_height),
        Constraint::Length(5),
        Constraint::Length(week_height),
        Constraint::Length(lfg_height),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, s, tall);
    draw_accounts(frame, accounts, s);
    draw_upload(frame, upload, s);
    draw_week(frame, week_area, week);
    if let Some(view) = &s.lfg {
        draw_lfg(frame, lfg, view);
    }
    draw_events(frame, events, s, ui.scroll);
    draw_footer(frame, footer, s, ui);

    if ui.mode != Mode::Normal {
        draw_input(frame, events, ui);
    }
}

fn panel(title: &str) -> Block<'_> {
    Block::bordered()
        .padding(Padding::horizontal(1))
        .border_style(Style::new().fg(Color::DarkGray))
        .title(Span::styled(format!(" {title} "), Style::new().fg(GOLD).bold()))
}

fn draw_header(frame: &mut Frame, area: Rect, s: &Status, with_crest: bool) {
    let (badge, colour) = badge(s);
    let block = panel(concat!("Murloc Defence League · desktop app ", env!("CARGO_PKG_VERSION")))
        .title_top(Line::from(Span::styled(format!(" {badge} "), Style::new().fg(Color::Black).bg(colour).bold())).right_aligned());
    let block = match &s.update {
        Some(u) => block.title_top(
            Line::from(Span::styled(format!(" v{} available ", u.version), Style::new().fg(Color::Black).bg(GOLD).bold()))
                .right_aligned(),
        ),
        None => block,
    };

    let dash = || "—".to_string();
    let mut first = vec![
        Span::styled("Signed in as  ", LABEL),
        Span::raw(format!("{:<26}", s.signed_in_as.clone().unwrap_or_else(dash))),
    ];
    if let Some(when) = countdown(s) {
        first.push(Span::styled("Next check  ", LABEL));
        first.push(Span::raw(when));
    }
    let mut lines = vec![
        Line::from(first),
        Line::from(vec![
            Span::styled("Game folder   ", LABEL),
            Span::raw(s.game_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_else(dash)),
        ]),
    ];
    if let Some(addon) = &s.addon {
        lines.push(addon_line(addon));
    }
    if let Some(e) = &s.last_error {
        lines.push(Line::from(vec![Span::styled("Problem       ", LABEL), Span::styled(e.clone(), Color::LightRed)]));
    }
    if with_crest {
        // The small crest on the left, the details beside it.
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let width = art::CREST_SMALL[0].len() as u16 + 1;
        let [crest, rest] = Layout::horizontal([Constraint::Length(width), Constraint::Min(10)]).areas(inner);
        let crest_lines: Vec<Line> =
            art::CREST_SMALL.iter().map(|l| Line::styled(*l, Style::new().fg(GOLD))).collect();
        frame.render_widget(Paragraph::new(crest_lines), crest);
        frame.render_widget(Paragraph::new(lines), rest);
    } else {
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }
}

/// "Addon  1.2.0 · 1.4.0 available" (step 13).
fn addon_line(a: &AddonView) -> Line<'static> {
    let mut spans = vec![Span::styled("Guild addon   ", LABEL)];
    match a.installed {
        Some(v) => spans.push(Span::raw(v.to_string())),
        None => spans.push(Span::styled("version not known", LABEL)),
    }
    if let Some(min) = a.too_old_for_murlocs {
        spans.push(Span::styled(format!(" · too old to send figures: needs {min}+"), Color::LightRed));
    } else if a.update_available {
        if let Some(latest) = a.latest {
            spans.push(Span::styled(format!(" · {latest} available"), Color::Yellow));
        }
    } else if a.installed.is_some() && a.latest.is_some() {
        spans.push(Span::styled(" · up to date", Color::Green));
    }
    if (a.update_available || a.too_old_for_murlocs.is_some()) && a.url.is_some() {
        spans.push(Span::styled(format!("  {}", a.url.clone().unwrap_or_default()), LABEL));
    }
    Line::from(spans)
}

/// The start-up splash: the crest, the name and the motto, centred in gold.
fn draw_splash(frame: &mut Frame) {
    let gold = Style::new().fg(GOLD);
    let mut lines: Vec<Line> = art::CREST.iter().map(|l| Line::styled(*l, gold).centered()).collect();
    lines.push(Line::raw(""));
    lines.push(Line::styled(art::NAME, gold.add_modifier(Modifier::BOLD)).centered());
    lines.push(Line::raw(""));
    lines.push(Line::styled(art::MOTTO, LABEL).centered());
    let height = lines.len() as u16;
    let area = frame.area();
    let top = area.y + area.height.saturating_sub(height) / 2;
    let area = Rect { y: top, height: height.min(area.height), ..area };
    frame.render_widget(Paragraph::new(lines), area);
}

/// The coloured badge text for the app's phase.
fn badge(s: &Status) -> (String, Color) {
    match s.phase {
        Phase::SetupNeeded => ("▲ Setup needed".into(), Color::Yellow),
        Phase::CheckingToken => ("… Checking token".into(), Color::Cyan),
        Phase::Watching => ("● Watching".into(), Color::Green),
        Phase::Uploading => ("⇡ Uploading".into(), Color::Cyan),
        Phase::Retrying => (format!("↻ Retrying {}", countdown(s).unwrap_or_default()), Color::Yellow),
        Phase::RateLimited => (format!("◷ Too many requests, waiting {}", countdown(s).unwrap_or_default()), Color::Yellow),
        Phase::Paused => ("■ Paused: press t to fix the token".into(), Color::LightRed),
    }
}

/// "in 12s" / "now", when there's a scheduled check worth showing.
fn countdown(s: &Status) -> Option<String> {
    if !matches!(s.phase, Phase::Watching | Phase::Retrying | Phase::RateLimited) {
        return None;
    }
    let left = s.next_check?.saturating_duration_since(Instant::now()).as_secs();
    Some(if left == 0 { "now".into() } else { format!("in {}", short_duration(left)) })
}

fn draw_accounts(frame: &mut Frame, area: Rect, s: &Status) {
    let width = s.accounts.iter().map(|a| a.name.len()).max().unwrap_or(0) + 3;
    let lines: Vec<Line> = if s.accounts.is_empty() {
        vec![Line::styled("No accounts with the addon's saved data yet.", LABEL)]
    } else {
        s.accounts
            .iter()
            .map(|a| {
                let (text, colour) = account_text(&a.state);
                Line::from(vec![Span::raw(format!("{:<width$}", a.name)), Span::styled(text, colour)])
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(lines).block(panel("Accounts")), area);
}

fn account_text(state: &AccountState) -> (String, Color) {
    match state {
        AccountState::Waiting => ("· not checked yet".into(), Color::DarkGray),
        AccountState::NoExport => (
            "! no murlocExport: this addon version doesn't send figures yet".into(),
            Color::Yellow,
        ),
        AccountState::UpToDate => ("✓ up to date (already sent)".into(), Color::Green),
        AccountState::Uploaded { at, changed } => {
            let what = if *changed { "new figures" } else { "website already had them" };
            (format!("✓ uploaded {} ago ({what})", ago(*at)), Color::Green)
        }
        AccountState::Rejected(msg) => (format!("✗ {msg}"), Color::LightRed),
        AccountState::ReadError(msg) => (format!("✗ {msg}"), Color::LightRed),
    }
}

fn draw_upload(frame: &mut Frame, area: Rect, s: &Status) {
    let lines = match &s.last_upload {
        None => vec![Line::styled("Nothing uploaded since the app started.", LABEL)],
        Some(u) => upload_lines(u),
    };
    frame.render_widget(Paragraph::new(lines).block(panel("Last upload")), area);
}

fn upload_lines(u: &Upload) -> Vec<Line<'static>> {
    let r = &u.result;
    let exported = Timestamp::from_second(r.exported_at)
        .map(|t| eventlog::local_time(t, "%H:%M %-d %b"))
        .unwrap_or_else(|_| "?".into());
    let (what, colour) = if r.changed { ("new figures", Color::Green) } else { ("no change", Color::DarkGray) };
    let mut lines = vec![
        Line::from(vec![
            Span::raw(format!("{} ago from {}", ago(u.at), u.account)),
            Span::styled(" · exported ", LABEL),
            Span::raw(format!("{exported} by {}", r.by)),
        ]),
        Line::from(vec![
            Span::raw(format!("{} weeks · {} people · {} top murlocs · ", r.weeks, r.people, r.tops)),
            Span::styled(what, colour),
        ]),
    ];
    if !r.unmatched.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Couldn't match: ", LABEL),
            Span::styled(r.unmatched.join(", "), Color::Yellow),
        ]));
    }
    lines
}

/// Phase 2: the next few runs, and how sign-ups are moving.
fn draw_lfg(frame: &mut Frame, area: Rect, view: &LfgView) {
    let last = view.last_poll.map_or("never".to_string(), |t| format!("{} ago", ago(t)));
    let mut lines = vec![Line::from(vec![
        Span::styled("Runs ", LABEL),
        Span::raw(view.runs_known.to_string()),
        Span::styled(" · sign-ups relayed ", LABEL),
        Span::raw(view.signups_sent.to_string()),
        Span::styled(" · waiting to send ", LABEL),
        Span::raw(view.pending.to_string()),
        Span::styled(" · last checked ", LABEL),
        Span::raw(last),
    ])];
    if view.upcoming.is_empty() {
        lines.push(Line::styled("No upcoming runs from the website yet.", LABEL));
    }
    for run in &view.upcoming {
        lines.push(run_line(run));
    }
    frame.render_widget(Paragraph::new(lines).block(panel("Group finder")), area);
}

/// "Sat 20:00  The Stockade  3/5" (with "demo" marked).
fn run_line(run: &Run) -> Line<'static> {
    let when = Timestamp::from_second(run.starts_at)
        .map(|t| eventlog::local_time(t, "%a %H:%M"))
        .unwrap_or_else(|_| "?".into());
    let mut spans = vec![
        Span::styled(format!("{when}  "), Style::new().fg(GOLD)),
        Span::raw(format!("{}  ", run.title)),
        Span::styled(format!("{}/{}", run.filled(), run.size), LABEL),
    ];
    if run.kind == "battleground" {
        spans.push(Span::styled("  battleground", LABEL));
    }
    if run.demo {
        spans.push(Span::styled("  demo", Color::Yellow));
    }
    Line::from(spans)
}

fn draw_events(frame: &mut Frame, area: Rect, s: &Status, scroll: usize) {
    let visible = area.height.saturating_sub(2) as usize;
    let end = s.events.len().saturating_sub(scroll);
    let start = end.saturating_sub(visible);
    let lines: Vec<Line> = s
        .events
        .range(start..end)
        .map(|e| {
            let (mark, colour) = match e.level {
                Level::Info => ("·", Color::Gray),
                Level::Ok => ("✓", Color::Green),
                Level::Warn => ("!", Color::Yellow),
                Level::Error => ("✗", Color::LightRed),
            };
            Line::from(vec![
                Span::styled(format!("{} ", e.clock()), LABEL),
                Span::styled(format!("{mark} "), colour),
                Span::styled(e.text.clone(), colour),
            ])
        })
        .collect();
    let hint = if scroll > 0 { " scrolled up · End to jump back " } else { " ↑↓ scroll " };
    let block = panel("Events").title_bottom(Line::styled(hint, LABEL).right_aligned());
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The "This week" panel's lines: the nudge (if any), then the top five.
fn week_lines(s: &Status) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let exported_at = s.report.as_ref().map(|r| r.exported_at);
    if let Some(text) = schedule::nudge(Timestamp::now(), exported_at) {
        lines.push(Line::styled(format!("! {text}"), Color::Yellow));
    }
    match &s.report {
        None => lines.push(Line::styled(
            s.report_note.clone().unwrap_or_else(|| "No murloc figures read yet.".into()),
            LABEL,
        )),
        Some(r) => match report::latest_week(r) {
            None => lines.push(Line::styled("The export has no weeks in it yet.", LABEL)),
            Some(week) => {
                let rows = report::leaderboard(r, week);
                if rows.is_empty() {
                    lines.push(Line::styled("Nobody's harmed a murloc this week. Yet.", LABEL));
                }
                let width = rows.iter().take(5).map(|r| r.character.chars().count()).max().unwrap_or(0) + 2;
                for (i, row) in rows.iter().take(5).enumerate() {
                    let mut spans = vec![
                        Span::styled(format!("{}. ", i + 1), Style::new().fg(GOLD)),
                        Span::raw(format!("{:<width$}", row.character)),
                        Span::styled(format!("{:>4}", row.kills), Style::new().add_modifier(Modifier::BOLD)),
                    ];
                    if let Some(m) = &row.top_murloc {
                        spans.push(Span::styled(format!("   favourite victim: {m}"), LABEL));
                    }
                    lines.push(Line::from(spans));
                }
                if rows.len() > 5 {
                    lines.push(Line::styled(format!("…and {} more", rows.len() - 5), LABEL));
                }
            }
        },
    }
    lines
}

fn draw_week(frame: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    let post = match schedule::next_discord_post(Timestamp::now()) {
        Some(at) => {
            let secs = at.timestamp().as_second() - Timestamp::now().as_second();
            format!(" Discord post in {} ", long_duration(secs.max(0) as u64))
        }
        None => " Discord countdown: step 12 isn't written yet ".into(),
    };
    let block = panel("This week's murloc slayers").title_top(Line::styled(post, LABEL).right_aligned());
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// "2d 4h", "3h 20m", "12m"
fn long_duration(s: u64) -> String {
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

fn draw_footer(frame: &mut Frame, area: Rect, s: &Status, ui: &Ui) {
    let fresh = ui.notice.as_ref().filter(|(_, at)| at.elapsed() < Duration::from_secs(6));
    let line = if let Some((text, _)) = fresh {
        Line::from(Span::styled(format!(" {text}"), Style::new().fg(GOLD)))
    } else {
        let key = |k: &'static str| Span::styled(k, Style::new().fg(GOLD).add_modifier(Modifier::BOLD));
        let mut spans = vec![
            Span::raw(" "),
            key("u"),
            Span::raw(" upload now  "),
            key("t"),
            Span::raw(" token  "),
            key("f"),
            Span::raw(" folder  "),
            key("d"),
            Span::raw(" detect  "),
            key("q"),
            Span::raw(" quit"),
        ];
        if let Some(u) = &s.update {
            spans.push(Span::styled(format!("   v{} is out: {}", u.version, u.url), Style::new().fg(GOLD)));
        } else if let Some(p) = &ui.log_path {
            spans.push(Span::styled(format!("   log: {}", p.display()), LABEL));
        }
        Line::from(spans)
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// A one-line input box drawn over the Events panel.
fn draw_input(frame: &mut Frame, over: Rect, ui: &Ui) {
    let (title, shown) = match ui.mode {
        Mode::EditToken => ("Paste the token from the website", "•".repeat(ui.input.chars().count())),
        _ => ("Game folder (the _forever_ folder)", ui.input.clone()),
    };
    let area = Rect { x: over.x + 2, y: over.y + 1, width: over.width.saturating_sub(4), height: 3 };
    let block = panel(title)
        .border_style(Style::new().fg(GOLD))
        .title_bottom(Line::styled(" Enter save · Esc cancel · Ctrl-U clear ", LABEL).right_aligned());
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(shown.clone()).block(block), area);
    // Put the terminal's cursor at the end of what's typed (clamped to the box).
    let x = (area.x + 2 + shown.chars().count() as u16).min(area.right().saturating_sub(2));
    frame.set_cursor_position((x, area.y + 1));
}

/// "5s", "3m", "2h", "4d" since a moment.
fn ago(t: SystemTime) -> String {
    short_duration(SystemTime::now().duration_since(t).unwrap_or_default().as_secs())
}

fn short_duration(s: u64) -> String {
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ImportResult;
    use crate::eventlog::Event;
    use crate::worker::AccountView;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn ui(mode: Mode, input: &str) -> Ui {
        Ui {
            tx: std::sync::mpsc::channel().0,
            token: String::new(),
            game_dir: None,
            mode,
            input: input.into(),
            scroll: 0,
            notice: None,
            log_path: None,
            splash_until: None,
            quit: false,
        }
    }

    fn sample() -> Status {
        let mut s = Status {
            phase: Phase::Watching,
            signed_in_as: Some("Shoop Dahoop".into()),
            game_dir: Some(PathBuf::from("/games/_forever_")),
            accounts: vec![
                AccountView { name: "130194931#4".into(), state: AccountState::Uploaded { at: SystemTime::now(), changed: true } },
                AccountView { name: "222#1".into(), state: AccountState::NoExport },
            ],
            last_upload: Some(Upload {
                at: SystemTime::now(),
                account: "130194931#4".into(),
                result: ImportResult {
                    changed: true,
                    exported_at: 1_795_219_200,
                    by: "Shoop Dahoop".into(),
                    weeks: 14,
                    people: 21,
                    tops: 9,
                    unmatched: vec!["Somebody Unlinked".into()],
                },
            }),
            next_check: Some(Instant::now() + Duration::from_secs(20)),
            ..Default::default()
        };
        s.events.push_back(Event::now(Level::Ok, "Uploaded 130194931#4: new figures"));
        s
    }

    /// Draws into an in-memory 110×32 screen and returns it as text.
    fn screen(s: &Status, ui: &Ui) -> String {
        screen_sized(s, ui, 110, 32)
    }

    fn screen_sized(s: &Status, ui: &Ui, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, s, ui)).unwrap();
        buffer_text(term.backend().buffer())
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn dashboard_shows_the_important_bits() {
        let text = screen(&sample(), &ui(Mode::Normal, ""));
        for want in [
            "● Watching",
            "Shoop Dahoop",
            "Next check",
            "/games/_forever_",
            "✓ uploaded",
            "! no murlocExport",
            "14 weeks · 21 people · 9 top murlocs",
            "Somebody Unlinked",
            "Uploaded 130194931#4: new figures",
            "upload now",
        ] {
            assert!(text.contains(want), "missing {want:?} in:\n{text}");
        }
    }

    #[test]
    fn problems_and_paused_are_shown() {
        let mut s = sample();
        s.phase = Phase::Paused;
        s.last_error = Some("The website doesn't recognise this token.".into());
        let text = screen(&s, &ui(Mode::Normal, ""));
        assert!(text.contains("Paused: press t to fix the token"));
        assert!(text.contains("The website doesn't recognise this token."));
    }

    #[test]
    fn token_input_is_masked() {
        let text = screen(&sample(), &ui(Mode::EditToken, "secret-token-123"));
        assert!(text.contains("Paste the token"));
        assert!(text.contains("••••••••••••••••"));
        assert!(!text.contains("secret-token-123"));
    }

    /// Prints the dashboard with sample data, to see a design change without
    /// running the app: `cargo test preview -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn preview() {
        let mut s = sample();
        s.events.push_front(Event::now(Level::Warn, "222#1: no murlocExport yet (this addon version doesn't send figures)"));
        s.events.push_front(Event::now(Level::Ok, "Signed in as Shoop Dahoop"));
        s.events.push_back(Event::now(Level::Info, "Watching 2 accounts for new murloc figures"));
        s.lfg = Some(LfgView {
            runs_known: 2,
            upcoming: vec![
                Run::from_value(&serde_json::json!({
                    "id": "r1", "kind": "dungeon", "title": "The Stockade", "starts_at": 1795230000, "size": 5,
                    "signups": [{"character": "Gotrek Rustpeak", "role": "tank", "status": "in"}]
                }))
                .unwrap(),
                Run::from_value(&serde_json::json!({
                    "id": "r2", "kind": "battleground", "title": "Warsong Gulch", "starts_at": 1795316400,
                    "size": 10, "demo": true, "signups": []
                }))
                .unwrap(),
            ],
            signups_sent: 4,
            pending: 1,
            last_poll: Some(SystemTime::now()),
        });
        s.update = Some(crate::update::UpdateInfo {
            version: crate::version::Version::parse("0.2.0").unwrap(),
            url: "https://github.com/ianleckey/mdl-officer-utils/releases/tag/v0.2.0".into(),
        });
        s.addon = Some(AddonView {
            installed: crate::version::Version::parse("1.2.0"),
            latest: crate::version::Version::parse("1.4.0"),
            update_available: true,
            too_old_for_murlocs: crate::version::Version::parse("1.3.0"),
            url: Some("https://murlocdefenseleague.org/addon".into()),
        });
        s.report_note = Some(crate::report::NOT_WRITTEN.into());
        println!("{}", screen(&s, &ui(Mode::Normal, "")));
        println!("{}", screen_sized(&s, &ui(Mode::Normal, ""), 110, 44));

        let mut term = Terminal::new(TestBackend::new(110, 24)).unwrap();
        term.draw(draw_splash).unwrap();
        println!("{}", buffer_text(term.backend().buffer()));
    }

    #[test]
    fn folder_input_is_plain() {
        let text = screen(&sample(), &ui(Mode::EditFolder, "/mnt/c/wow/_forever_"));
        assert!(text.contains("/mnt/c/wow/_forever_"));
    }
}
