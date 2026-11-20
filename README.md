# MDL officer utils

The desktop app for **Murloc Defence League** officers. It carries data between the guild addon in the game and the guild website, so nobody has to copy and paste it by hand.

```
website  <-- HTTPS -->  this app  <-- files -->  guild addon (in game)
```

The game's addons can't use the network. Instead, this app watches the files the game writes, and talks to the website on the addon's behalf.

> **Status: alpha.** Impact Report sync works. The group finder (phase 2) is built but switched off until the website and the addon support it.

## What it does

- **Impact Report sync.** When you log out or `/reload`, the game saves the addon's murloc figures. Within 30 seconds the app uploads them to the website, so there's no more pressing *Export for website* and pasting. It only sends an export when it has changed, and it's safe to run alongside other officers: the website keeps the higher counts.
- **A dashboard in your terminal.** It shows:
  - who you're signed in as;
  - every game account it found, and what it last did with each;
  - your last upload (weeks, people, any names the website couldn't match);
  - a scrollable event history.
- **Clear reasons when nothing happens.** For example, an account whose addon is too old to send figures is flagged as such, rather than the app sitting there silently.
- **An event log** in `events.log`, next to the settings, for working out later what happened.
- **Coming:**
  - the group finder: runs from the website show in game, and sign-ups go back;
  - this week's murloc leaderboard;
  - a countdown to the Wednesday 19:00 Discord post;
  - an addon version check.

## Install

Only officers need this. Everyone else just needs the addon.

1. Download the latest build from **[Releases](https://github.com/ianleckey/mdl-officer-utils/releases)**:
   - Windows: `mdl-desktop-windows-x86_64.exe`
   - Linux: `mdl-desktop-linux-x86_64` (then `chmod +x` it)
2. Run it.
   - **Windows:** double-click it. It opens in a console window. The `.exe` isn't code-signed, so Windows SmartScreen may say *"Windows protected your PC"*. Click **More info → Run anyway**.
   - **Linux:** run it from a terminal.

The app tells you when a newer version is out (a gold badge at the top). It never updates itself.

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
cargo test                # tests (some fail on purpose; see below)
cargo build --release     # target/release/mdl-desktop
```

**Windows `.exe` from Linux or WSL:**

```sh
sudo apt install mingw-w64
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

**Releases:** push a `v*` tag (e.g. `v0.2.0`) and GitHub Actions builds the Linux and Windows binaries and attaches them to a release. Tags with a hyphen (`v0.2.0-alpha.1`) become pre-releases. The full steps are in [GUIDE.md](GUIDE.md#releasing).

## Developing

This is also a Rust learning project. **[GUIDE.md](GUIDE.md)** walks through the code step by step, with hints, and several modules are deliberately left as skeletons with their tests already written. Those tests fail until each step is done, and CI skips them until then.

- **`tools/mock_site.py`** is a fake website (Python, standard library only) that speaks the API, including endpoints the real site doesn't have yet. Point the app at it with `api_base = "http://127.0.0.1:8787/api/v1"` in `config.toml`.
- **`cargo test preview -- --ignored --nocapture`** prints the dashboard with sample data, so you can check a layout change without running the app.

The contract between the website, this app and the addon is the *LFG desktop sync / desktop app* spec. If they disagree, the spec wins: change it there first, then in code.
