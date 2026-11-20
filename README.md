# MDL officer utils

The desktop app for **Murloc Defence League** officers. It carries data between the guild addon in the game and the guild website, so nobody has to copy and paste it by hand.

```
website  <-- HTTPS -->  this app  <-- files -->  guild addon (in game)
```

The game's addons can't use the network. Instead, this app watches the files the game writes, and talks to the website on the addon's behalf.

> **Status: alpha.** Impact Report sync works. The group finder (phase 2) is built but switched off until the website and the addon support it.

## What it does

- **Impact Report sync.** When you log out or `/reload`, the game saves the addon's murloc figures. Within 30 seconds the app uploads them to the website, so there's no more pressing *Export for website* and pasting. It only sends an export when it has changed, and it's safe to run alongside other officers: the website keeps the higher counts.
- **A dashboard in your terminal**, made for a big or maximised window:
  - **Header:** who you're signed in as, the game folder, the guild addon's version,
    and the website's health: which site it's talking to, when it last answered and
    how fast, and how each feature is doing (working, unreachable, busy, or not on
    the site yet).
  - **Murloc figures:** every game account and what it last did, your last upload
    (weeks, people, any names the website couldn't match), and this week's leaderboard.
  - **Group finder** (when it's switched on), beside the murloc panel on wide
    terminals: sign-ups relayed and waiting, and as many upcoming runs as fit.
  - **Events:** a scrollable history across the full width.
- **Clear reasons when nothing happens.** For example, an account whose addon is too old to send figures is flagged as such, rather than the app sitting there silently.
- **An event log** in `events.log`, next to the settings, for working out later what happened.
- **This week's murloc slayers:** the top names from the newest export, and a
  Wednesday reminder to log in and out before the 19:00 Discord post if the
  closed week's figures haven't been exported yet.
- **An addon version check:** the installed guild addon's version in the
  header, and whether the website has a newer one (once the website publishes it).
- **Coming:** the group finder: runs from the website show in game, and sign-ups go back.

## Install

Only officers need this. Everyone else just needs the addon.

1. Download the latest build from **[Releases](https://github.com/ianleckey/mdl-officer-utils/releases)**:
   - Windows: `mdl-desktop-windows-x86_64.exe`
   - Linux: `mdl-desktop-linux-x86_64` (then `chmod +x` it)
2. Run it.
   - **Windows:** double-click it. It opens in a console window. The `.exe` isn't code-signed, so Windows SmartScreen may say *"Windows protected your PC"*. Click **More info → Run anyway**.
   - **Linux:** run it from a terminal.

**Updates install themselves.** Once a day (or when you press `c`) the app checks GitHub for a newer version (test versions included), downloads it, checks it isn't corrupted, and puts it in place. A gold **ready: press r to restart** badge appears; press `r` to switch over (it waits for any upload in progress), or just carry on and the new version starts next time. If it can't update itself (say the folder it lives in is read-only), the badge links to the download instead.

The first version that can do this is 0.2.0-alpha.2. Anyone on an older copy needs to download a new one by hand, once.

## First run

1. On the website, go to the officer pages → **Desktop app** and create a token. It's shown once, so copy it.
2. In the app, press **`t`**, paste the token, and press **Enter**. The app checks it with the website.
3. Press **`d`** to find the game folder. If it can't, press **`f`** and type the path to the `_forever_` folder:
   - **Windows:** `C:\Program Files (x86)\World of Warcraft\_forever_`
   - **Linux (Wine / Faugus):** the same path inside the Wine prefix, e.g. `~/Faugus/battlenet/drive_c/Program Files (x86)/World of Warcraft/_forever_`
4. Play as normal. Whenever you log out or `/reload`, your figures go up within half a minute.

Leave the app running while you play. Closing it just means figures wait until you next open it.

## Keys

| Key | Does |
|---|---|
| `u` | Check the game's files now, instead of waiting for the next check |
| `t` | Set the website token (shown as dots) |
| `f` | Set the game folder by typing it |
| `d` | Find the game folder automatically |
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | Scroll the events |
| `c` | Check for a newer version of the app now (instead of waiting for the daily check) |
| `r` | Restart into an update that's been installed |
| `q` / `Ctrl-C` | Quit |

## What it touches

- **Reads** `WTF/Account/*/SavedVariables/MurlocDefenceLeague.lua`, for every game account. It **never writes** to these files: the game owns them.
- **Writes** (phase 2 only, when it's switched on) one small data addon, `Interface/AddOns/MurlocDefenceLeague_Data/`. That's all it ever writes in the game folder.
- **Keeps its own settings** outside the game folder, readable only by you on Linux:

  | OS | Folder |
  |---|---|
  | Windows | `%APPDATA%\MurlocDefenceLeague\mdl-desktop\config\` |
  | Linux | `~/.config/mdl-desktop/` |

  The folder holds `config.toml` (your token and game folder), `events.log`, and a lock file that stops two copies running at once.

Your token is like a password: don't share `config.toml`. If it leaks, revoke the token on the website and make a new one.

## Building from source

You need [Rust](https://rustup.rs) (stable).

```sh
cargo run                 # the dashboard
cargo test                # tests
cargo build --release     # target/release/mdl-desktop
```

**Windows `.exe` from Linux or WSL:**

```sh
sudo apt install mingw-w64
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

The result is `target/x86_64-pc-windows-gnu/release/mdl-desktop.exe`. To try it from WSL, copy it somewhere under `/mnt/c/` and run it there: it behaves like a real Windows program (real `C:\` paths, config in `%APPDATA%`). `mdl-desktop.exe --version` checks a build without opening the dashboard. If the build fails compiling `aws-lc-sys`, check `x86_64-w64-mingw32-gcc --version` works; you may also need `sudo apt install cmake nasm`.

## Releasing

1. Bump `version` in `Cargo.toml` (e.g. `0.2.0`), then run `cargo build` so `Cargo.lock` agrees.
2. Commit, then tag and push:
   ```sh
   git tag v0.2.0
   git push origin master v0.2.0
   ```
3. A few minutes later the Releases page has `mdl-desktop-linux-x86_64` and `mdl-desktop-windows-x86_64.exe`. Tags with a hyphen (`v0.2.0-alpha.1`) become pre-releases.

Each binary gets a `.sha256` file next to it; running copies (0.2.0-alpha.2 and later) check GitHub once a day, download the newest release for their platform, check it against that file, and install it. Pre-releases count, so every officer's copy picks up an alpha. To try a release build without releasing, run the *Release* workflow by hand from the Actions tab: the binaries are attached to the run.

## Developing

```
 main thread (dashboard)            worker thread
 ───────────────────────            ─────────────
 draws Status     <── Arc<Mutex<Status>> ── writes Status (and events)
 keys u, t, f, d  ── mpsc::Sender<Command> ──> recv_timeout(...)
```

The dashboard (`tui.rs`) redraws four times a second and never waits on the network or disk. The worker (`worker.rs`) wakes every 30 seconds, reads each account's SavedVariables file when it changes (`savedvars.rs`, `game.rs`), uploads new exports (`api.rs`), and in phase 2 writes the data addon (`lfg.rs`, `lua.rs`, `data_addon.rs`). It is the only thing that saves `config.toml`. The "This week" panel comes from `report.rs` and `schedule.rs`, the addon check from `addon.rs`, the app update check from `update.rs`.

- **`tools/mock_site.py`** is a fake website (Python, standard library only) that speaks the API, including endpoints the real site doesn't have yet. Its docstring lists switches to simulate a revoked token, the site being down, or no group finder. Point the app at it in `config.toml`:
  ```toml
  api_base = "http://127.0.0.1:8787/api/v1"
  token = "test"
  lfg_enabled = true   # the group finder; off by default
  ```
  `cargo test api::mock -- --ignored` checks every endpoint against it.
- **`cargo test preview -- --ignored --nocapture`** prints the dashboard with sample data, so you can check a layout change without running the app.
- CI builds, runs clippy and runs every test on each push.

The contract between the website, this app and the addon is the *LFG desktop sync / desktop app* spec. If they disagree, the spec wins: change it there first, then in code.
