# Building the MDL desktop app

This is a map of the app, not the code for it. Each step says what the piece is for, what goes in and out, and the edge cases the spec cares about. It ends with a **"you're done when…"** check. If you get stuck, open the hints under **Stuck?** They point you at what to look up and stop short of the answer, and they go from gentle to more specific.

The spec is `MDL-desktop-app-spec.pdf`. Where this guide and the spec disagree, the spec wins.

---

## 0. The big picture

```
website  <-- HTTPS -->  this app  <-- files -->  the addon (in game)
```

The addon can't use the network, so the app is the go-between. In **phase 1** it carries traffic one way only:

1. The officer logs out or types `/reload`. The game saves the addon's data into
   `WTF/Account/<ACCOUNT>/SavedVariables/MurlocDefenceLeague.lua`.
2. Inside that file, one line holds `["murlocExport"] = "MDLMURLOCS;1;…"`. That's the same text the in-game *Export for website* window shows.
3. The app notices the file changed and pulls that string out. If the string is different from the last one it sent, it `POST`s it to `/murlocs`.
4. The website updates the Impact Report. On Wednesdays at 19:00 the site posts the report to Discord, and the app has nothing to do with that part.

The app never writes to that file. The game owns it.

### How the code is laid out

| File | What it is | State |
|---|---|---|
| `main.rs` | Starts everything: loads config, spawns the worker thread, runs the dashboard | done |
| `config.rs` | Token, game folder and export hashes, saved as TOML in your OS config folder | done |
| `tui.rs` | The full-screen dashboard (ratatui). It reads keys, draws `Status` and sends `Command`s | done |
| `eventlog.rs` | The event history: the Events panel, plus `events.log` next to config.toml | done |
| `savedvars.rs` | Pulls one string out of the SavedVariables file | **you: step 1** (tests already written) |
| `game.rs` | Finds the game folder and every account's file | **you: step 2** |
| `api.rs` | Talks to the website | **you: step 3** (types done, two functions to write) |
| `worker.rs` | The loop that ties it all together | **you: step 4** (loop done, `start` and `tick` to write) |
| `art.rs` | The guild crest: start-up splash, and the header on tall terminals | done |
| `lua.rs` | Phase 2: JSON → Lua source for Data.lua | **you: step 6** (tests already written) |
| `data_addon.rs` | Phase 2: the data addon's folder, .toc and Data.lua | **you: step 7** (tests already written) |
| `lfg.rs` | Phase 2: the group finder's types (done) and four helpers | **you: step 8** (tests already written) |
| `api.rs` (again) | Phase 2: `get_lfg()` and `post_signups()` | **you: step 9** |
| `worker.rs` (again) | Phase 2: `lfg_start()`, `poll_lfg()`, `relay_signups()` | **you: step 10** (wiring done) |
| `tools/mock_site.py` | A fake website for testing phase 2 before the real one exists | done |
| `version.rs` / `update.rs` | Version numbers, and the "newer app version" check against GitHub releases | done |
| `report.rs` | Reads the export for the "This week" leaderboard | **you: step 11** (tests already written) |
| `schedule.rs` | The weekly reset and the Wednesday 19:00 Discord countdown | **you: step 12** (tests already written) |
| `addon.rs` (+ a function each in `api.rs` and `game.rs`) | Is the installed guild addon up to date? | **you: step 13** (tests already written) |
| `.github/workflows/` | CI on every push; Linux and Windows release builds on every `v*` tag | done |

**Why two threads?** The main thread redraws the dashboard four times a second and reacts to keys. The worker has to wake every 30 seconds, and it may sit waiting on the network. Splitting them means neither blocks the other. The worker writes results into a shared `Status` (including the event history), and the dashboard draws whatever is in it. The dashboard talks to the worker through a channel (`Command::UploadNow` and so on).

```
 main thread (dashboard)            worker thread
 ────────────────────               ─────────────
 draws Status     <── Arc<Mutex<Status>> ── writes Status (and events)
 keys u, t, f, d  ── mpsc::Sender<Command> ──> recv_timeout(...)
```

### Commands you'll use

```sh
cargo run                    # start the dashboard (u upload, t token, f folder, d detect, q quit)
cargo test preview -- --ignored --nocapture   # print the dashboard with sample data
cargo test                   # run every test
cargo test savedvars         # just the step 1 tests
cargo clippy                 # extra lints, worth running now and then
```

---

## Step 1: `savedvars::read_key`

**Job:** take the whole text of `MurlocDefenceLeague.lua` and a key name (`"murlocExport"`), and give back the string stored under that key with Lua's escapes undone.

**The rules (from the spec):**
1. Find the line that **starts with** `["murlocExport"] = "`. The game indents with tabs, so ignore leading whitespace. The pattern only counts at the start of a line, not in the middle of some other value.
2. From there, read characters up to the closing `"` that is **not** escaped.
3. Undo the escapes as you go:
   - `\\` → `\`
   - `\"` → `"`
   - `\n` → newline
   - `\r` → carriage return
   - `\t` → tab
   - `\ddd` → the byte with that decimal value, 1 to 3 digits (`\195\169` is two bytes, which together make `é`)
4. The result is UTF-8.

**Outcomes:**
- Key not there → `Ok(None)`. This is normal, not an error: an older addon, or nothing to report yet.
- Found it → `Ok(Some(text))`.
- Found it but it's broken (no closing quote, unknown escape, not valid UTF-8) → `Err(message)`.

**You're done when:** `cargo test savedvars` shows 13 passed.

<details><summary>Stuck? Finding the line</summary>

- `str::lines()` gives you each line, and it already strips `\r\n` endings.
- `str::trim_start()` drops the leading tabs.
- `str::strip_prefix(&pattern)` returns `Some(rest_of_line)` only if the line starts with the pattern. That covers the "must start the line" rule for free.
- Build the pattern with `format!`. Getting the quotes inside it right is fiddly. A raw string like `r#"…"#` doesn't help, because you need `{key}` in it, so use `\"` inside a normal `format!` string.
</details>

<details><summary>Stuck? Undoing escapes</summary>

- `\195\169` gives you two separate **bytes** that only make sense together. You can't turn each one into a `char` on its own. So collect everything into a `Vec<u8>` and turn it into a `String` once at the end with `String::from_utf8`. Ordinary text that's already UTF-8 (like `é` typed straight in) can be pushed byte by byte too.
- Walking the string as bytes (`rest.as_bytes()`) with an index `i` and a `loop` is the easiest shape. Each step looks at `bytes[i]`: is it `"` (done), `\` (look at the next byte), or anything else (push it)?
- For `\ddd`: while the next byte is an ASCII digit and you've read fewer than 3 digits, do `value = value * 10 + digit`. Use a `u32` for `value` so 3 digits can't overflow, then check it fits in a `u8` (`u8::try_from`).
- Running out of bytes before finding the closing `"` → that's the `Err` case.
</details>

<details><summary>Stuck? Testing a single case</summary>

`cargo test savedvars::tests::short_decimal_escape -- --nocapture` runs one test and shows any `println!`/`dbg!` output.
</details>

---

## Step 2: `game.rs`

**Job:** know where the game is, and list every account's saved-data file.

### `looks_like_game_dir(dir)`
True if `dir/Interface/AddOns` or `dir/WTF` exists. This guards against the officer choosing the wrong folder.

### `detect()`
Try the usual places and return the first that `looks_like_game_dir`:
- **Windows:** `C:\Program Files (x86)\World of Warcraft\_forever_`
- **Linux (Wine/Faugus):** the same path under a Wine prefix's `drive_c`. Worth trying:
  - `~/Faugus/<anything>/drive_c/Program Files (x86)/World of Warcraft/_forever_`
  - `~/.wine/drive_c/…same…`
  - `~/Games/<anything>/drive_c/…same…` (Lutris)

  `<anything>` means "look at every folder in there", because each Faugus/Lutris prefix has its own name.

### `saved_var_files(game_dir)`
For every folder in `WTF/Account/`, check whether `SavedVariables/MurlocDefenceLeague.lua` exists inside it. If it does, return it with the account folder's name (e.g. `130194931#4`). Skip anything that isn't a folder (there's a `SavedVariables` folder directly under `Account` in some installs too, so it's fine if that just doesn't match). Sort the results so the order is stable.

**You're done when:**
- your own test passes: build a fake `_forever_` in a temp folder with two accounts, where only one has the file, and check you get exactly one back;
- pressing `d` in the dashboard finds your real game folder.

<details><summary>Stuck? Paths and folders</summary>

- `Path::join` builds paths and handles `/` versus `\` for you. Join one piece at a time: `.join("WTF").join("Account")`.
- `Path::is_dir()` / `Path::is_file()` / `Path::exists()`.
- `std::fs::read_dir(path)` iterates the entries in a folder. Each entry is a `Result`, and `entry.path()` / `entry.file_name()` are what you want.
- `file_name()` is an `OsString`. `.to_string_lossy().into_owned()` gets you a `String`.
- The home folder: `directories::BaseDirs::new()` then `.home_dir()`. The crate is already a dependency.
- `#[cfg(windows)]` / `#[cfg(not(windows))]` on a block or `let` lets you have different defaults per OS.
</details>

<details><summary>Stuck? Writing the test</summary>

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_accounts() {
        let tmp = tempfile::tempdir().unwrap();
        // std::fs::create_dir_all(...) to make folders, std::fs::write(...) to make the file
        // then call saved_var_files(tmp.path()) and assert_eq! on the result
    }
}
```
`tempfile` deletes the folder when `tmp` goes out of scope.
</details>

---

## Step 3: `api.rs`: `me()` and `post_murlocs()`

**Job:** two HTTP calls, plus turning every way they can fail into the right `ApiError`.

Every request:
- carries the header `Authorization: Bearer <token>`;
- sends and receives JSON.

| Call | Request | Success |
|---|---|---|
| `me()` | `GET {base}/me` | 200 `{"name": "...", "officer": true}` → `Me` |
| `post_murlocs(export)` | `POST {base}/murlocs` with body `{"export": "<the text exactly as read>"}` | 200 → `ImportResult` |

**Errors (same for both):**

| What happened | `ApiError` |
|---|---|
| 401 | `Unauthorized` |
| 403 | `NotOfficer` |
| 422 | `Rejected(msg)`, where `msg` is the `"error"` field of the JSON body |
| 429 | `RateLimited(d)`, where `d` comes from the `Retry-After` header (seconds; default to 60 if missing or unreadable) |
| 5xx, couldn't connect, timed out, 200 but the JSON didn't fit | `Transient(description)` |

Write one helper that takes a response that wasn't a 200 and returns the matching `ApiError`, so both calls share it.

**You're done when:** with a real token from the website saved with the `t` key, `me()` returns your name. Before step 4 you can test this with a quick `#[test] #[ignore]` that reads the token from an env var. `cargo test -- --ignored` runs it.

<details><summary>Stuck? Making the request</summary>

- `self.http.get(url)` / `self.http.post(url)` give you a `RequestBuilder`.
- `.bearer_auth(&self.token)` adds the header.
- `.json(&value)` sets the body and the `Content-Type`. `serde_json::json!({ "export": export })` builds the value.
- `.send()` returns `Result<Response, reqwest::Error>`. An `Err` here means the network failed, so map it to `Transient(e.to_string())`.
</details>

<details><summary>Stuck? Reading the response</summary>

- `response.status()` gives a `StatusCode`. `.as_u16()` gives you a number to `match` on, and `.is_server_error()` covers all 5xx.
- `response.json::<Me>()` parses the body into your struct.
- For 422, define a tiny `#[derive(Deserialize)] struct ErrorBody { error: String }` and parse into that.
- `response.headers().get(reqwest::header::RETRY_AFTER)` returns `Option<&HeaderValue>`. Then `.to_str().ok()?.parse::<u64>().ok()` (inside a helper that returns `Option`) turns it into seconds.
- `response.json()` consumes the response, so read the status (and the headers) first.
</details>

---

## Step 4: `worker.rs`: `start()` and `tick()`

The loop (`run`) is already written. It calls `start()` at launch and after every token or folder change. It calls `tick()` every 30 seconds and when you press `u`. Each function has a numbered comment in the code; this is the longer version.

### `start()`: get ready
1. **Reset.** Clear `client`, `files`, `last_seen`, `retry_after` and `backoff`, and set `paused = false` in the status. A new token or folder means starting over.
2. **No token** (`config.token` empty)? Set `activity` to something like "No token yet: press t and paste one from the website" and stop.
3. **Check the token.** Make an `api::Client` and call `me()`.
   - `Ok` with `officer: true` → `signed_in_as = Some(name)`, keep the client.
   - `Ok` with `officer: false`, `Unauthorized` or `NotOfficer` → `paused = true`, put the reason in `last_error`, stop.
   - `Transient` / `RateLimited` → show the error, but keep going. The token might be fine and the site just down; `tick` will retry.
4. **Find the files.** Game folder = `config.game_dir`, or if that's `None`, `game::detect()`. Neither works? Say "Choose the game folder" and stop. Otherwise call `saved_var_files`, store them in `self.files`, and put the folder and account names in the status.
5. **Go.** `next_check = Instant::now()` so the first `tick` happens at once (and uploads anything new since last time).

### `tick()`: the 30-second check
1. **Should I even?** If paused, or `retry_after` is still in the future, set the next check and return.
2. **For each file:**
   1. Get its modified time. If it's the same as `last_seen[path]`, skip it: nothing changed since we last read it.
   2. Read the file and call `read_key(text, "murlocExport")`. `None` → nothing to do (remember the mtime and move on). `Err` → show it, move on.
   3. **Hash** the export (SHA-256, as hex). If it equals `config.export_hashes[account]`, it's already been sent, so remember the mtime and move on.
   4. **Upload** with `post_murlocs`, then:

      | Result | Do this |
      |---|---|
      | `Ok(result)` | Store the hash in `config.export_hashes` and `save_config()`. Set `last_upload`, clear `last_error`, reset `backoff` to `None`, remember the mtime |
      | `Rejected(msg)` | **Still store the hash** (the spec says sending the same text again won't help), show `msg`, remember the mtime |
      | `Unauthorized` / `NotOfficer` | `paused = true`, show why, stop the tick |
      | `RateLimited(d)` | `retry_after = now + d`, stop the tick |
      | `Transient(msg)` | Show it. **Don't** store the hash or the mtime (so it's tried again). Double `backoff` (start at 30 s, cap at `MAX_BACKOFF`), stop the tick |
3. **Schedule.** `next_check = now + backoff` if backing off, else `now + CHECK_EVERY`.

Why remember the mtime *and* the hash? The mtime is a cheap "did the file change?" check that avoids reading it every 30 seconds. The hash answers "is this export different from what we sent?", and it survives restarts because it lives in config.toml. A `/reload` changes the mtime even when the figures don't change, so the hash catches that.

**You're done when:**
- After saving a token, the dashboard shows your name, folder and accounts.
- Log in to the game, kill a murloc, `/reload`, and within ~30 s an "Uploaded …" event appears. `/reload` again without doing anything: no new upload (hash matched).
- Restart the app: no re-upload (hashes came back from config.toml).
- Pull the network (or point `api_base` in config.toml at `http://127.0.0.1:9`): the error shows, and the app keeps retrying, more slowly each time.

<details><summary>Stuck? File times and hashing</summary>

- `std::fs::metadata(path)?.modified()?` gives a `SystemTime`, which can be compared with `==`.
- `std::fs::read_to_string(path)` reads the file. On Linux the game may be writing it at the very moment you read. If `read_key` says "unterminated", the simplest fix is to not record the mtime, so the next tick tries again.
- Hashing: `use sha2::{Digest, Sha256};` then `Sha256::digest(text.as_bytes())`. For hex, loop over the bytes with `format!("{:02x}", b)`, or `.iter().map(|b| format!("{b:02x}")).collect::<String>()`.
</details>

<details><summary>Stuck? The borrow checker in `tick`</summary>

Looping over `self.files` while calling `&mut self` methods inside the loop won't compile ("cannot borrow `*self` as mutable because it is also borrowed as immutable"). The easy way out is `for file in self.files.clone() { … }`. The list is tiny, so cloning it costs nothing. Another way is `std::mem::take(&mut self.files)` before the loop and putting it back after.
</details>

<details><summary>Stuck? Backoff arithmetic</summary>

`Duration` has `saturating_mul` / `min`: `let next = self.backoff.map_or(CHECK_EVERY, |b| (b * 2).min(MAX_BACKOFF));`. Start from 30 s; after 30, 60, 120, 240, 480, 960 it caps at 900.
</details>

---

## Step 5: polish (optional)

Done:
- The full-screen dashboard (`tui.rs`): state badge, countdown to the next check, per-account state, last upload with `exported_at`/`by`/`tops`, a scrollable Events panel, and a masked token input.
- Per-account "no murlocExport" warning, logged once rather than every 30 seconds.
- The event log: the Events panel, plus `events.log` next to config.toml (rotated at 1 MB).

Still open:
- Start with Windows: a shortcut in `shell:startup` is enough. No code needed.
- Retry `/me` when the website was down at start-up (today you'd press `t` and re-save the token, or restart).

---

## Building for Windows from WSL

One-time setup:
```sh
sudo apt install mingw-w64
rustup target add x86_64-pc-windows-gnu
```
`.cargo/config.toml` already points Cargo at the mingw linker.

Build:
```sh
cargo build --release --target x86_64-pc-windows-gnu
```
The result is `target/x86_64-pc-windows-gnu/release/mdl-desktop.exe`, a single file you can hand to Reverend Drew.

To try it on Windows straight from WSL, copy it somewhere on `C:` and run it. WSL can launch Windows programs:
```sh
cp target/x86_64-pc-windows-gnu/release/mdl-desktop.exe /mnt/c/Users/<you>/Desktop/
/mnt/c/Users/<you>/Desktop/mdl-desktop.exe
```
It runs in your WSL terminal but behaves like a real Windows program: real `C:\Program Files (x86)\…` paths, and config saved in `%APPDATA%`. Double-clicking the .exe on Windows opens it in a console window.

If the build fails while compiling `aws-lc-sys` (the TLS crypto library), check that `x86_64-w64-mingw32-gcc --version` works. You may also need `sudo apt install cmake nasm`.

To check a build without opening the dashboard: `mdl-desktop.exe --version`.

---

## Releasing

The repo builds and publishes binaries for you (`.github/workflows/release.yml`):

1. Bump `version` in `Cargo.toml` (e.g. `0.2.0`), then run `cargo build` so `Cargo.lock` agrees.
2. Commit, then tag and push:
   ```sh
   git tag v0.2.0
   git push origin master v0.2.0
   ```
3. A few minutes later the GitHub Releases page has `mdl-desktop-linux-x86_64` and `mdl-desktop-windows-x86_64.exe`.

Every running copy of an older version checks GitHub once a day and shows a gold **v0.2.0 available** badge, with the release link in the footer. It never downloads anything by itself.

**Trying the release build without releasing:** on GitHub, open Actions → Release → *Run workflow*. That builds both binaries and attaches them to the run as downloadable artifacts, with no release made.

**CI** (`.github/workflows/ci.yml`) builds, runs clippy, and runs the tests on every push. The tests for unfinished skeleton steps are skipped there with `--skip …`; delete each `--skip` line once you've finished that step, so CI keeps it passing.

**Single copy:** the app takes a lock (`mdl-desktop.lock` next to config.toml), so a second copy says "already running" and quits. The OS drops the lock if the app crashes, so it can't get stuck.

---

## Phase 2: the group finder

Neither the website's endpoints (`GET /lfg`, `POST /lfg/signups`) nor the addon side exist yet, so everything here sits behind a switch in `config.toml`:

```toml
lfg_enabled = true   # default false
```

With it off, the app behaves exactly as in phase 1. With it on before you've written step 10, the worker only logs "step 10 … isn't written yet", so it's safe to flip whenever you like.

**The plan, in one picture:**

```
 GET /lfg (every 5 min, ETag)  ──>  LfgSnapshot  ──┐
                                                    ├──>  Data.lua  ──>  addon (at /reload)
 POST /lfg/signups results  ──>  acks (7 days)  ───┘
        ^
        └──  lfgOutbox in SavedVariables (read whenever the file changes, like murlocExport)
```

**Testing without the real site:** `tools/mock_site.py` is a fake website that speaks the spec's API:

```sh
python3 tools/mock_site.py          # http://127.0.0.1:8787
```

Then put this in `~/.config/mdl-desktop/config.toml`:

```toml
api_base = "http://127.0.0.1:8787/api/v1"
token = "test"
lfg_enabled = true
```

Its docstring lists which sign-ups it rejects, and why (to exercise every `reason`), plus switches to simulate a revoked token, the site being down, or no group finder (404).

The spec still has three open questions: battleground roles, desktop notifications, and demo runs. The app passes runs through to the addon untouched, so none of them need code here. The dashboard just marks demo runs and battlegrounds.

---

## Step 6: `lua.rs`: `escape_str` and `to_lua`

**Job:** turn a `serde_json::Value` into Lua source text. This is the reverse of step 1.

The rules are in the comment at the top of `lua.rs`. In short:
- **Strings:** quoted. `\` becomes `\\`, `"` becomes `\"`, newline becomes `\n`, and other control characters become a 3-digit `\ddd`. UTF-8 passes through as-is.
- **Objects:** `{ key = value, }` when the key is a plain Lua name, `{ ["odd-key"] = value, }` otherwise. Nulls are skipped.
- **Arrays:** `{ value, value, }`.

**You're done when:** `cargo test lua` passes (11 tests). Several of them load your output into a real Lua and check it reads back to the same JSON, so "looks right" isn't enough. It has to *be* right.

<details><summary>Stuck? Escaping</summary>

- Loop over `s.chars()`, push into a `String` that starts with `"` and ends with `"`.
- `match c { '\\' => ..., '"' => ..., '\n' => ..., c if (c as u32) < 32 || c as u32 == 127 => ..., c => out.push(c) }`
- Three-digit decimal: `format!("\\{:03}", c as u32)`. `{:03}` pads with zeros to 3 places.
</details>

<details><summary>Stuck? The recursion</summary>

- `match value { Value::Null => ..., Value::Bool(b) => ..., Value::Number(n) => n.to_string(), Value::String(s) => escape_str(s), Value::Array(items) => ..., Value::Object(map) => ... }`
- For arrays and objects, call `to_lua` on each child. That's the recursion.
- "Plain Lua name": not empty, doesn't start with a digit, every char is `is_ascii_alphanumeric()` or `_`, and not in `KEYWORDS` (`KEYWORDS.contains(&key.as_str())`).
- Building the pieces into a `Vec<String>` and then `.join(...)` is tidier than pushing commas by hand.
- Indentation is optional. The tests don't check it, but a readable Data.lua is nice when debugging. If you want it, add a `depth: usize` parameter in a private helper and call that from `to_lua`.
</details>

---

## Step 7: `data_addon.rs`

**Job:** everything about the tiny addon the app owns, `Interface/AddOns/MurlocDefenceLeague_Data/`. The paths and the `.toc` text are done. You write four functions:

| Function | What it does |
|---|---|
| `interface_version(game_dir)` | Read the main addon's `.toc` and return the number after `## Interface:` |
| `render_data(snapshot, acks)` | Build the whole Data.lua text: `MurlocDefenceLeagueData = { version = 1, generated_at = …, runs = {…}, acks = {…} }` |
| `write_if_changed(path, text)` | Same text already there → `Ok(false)`. Otherwise create the folder, write `Data.lua.tmp`, rename it over the real file, `Ok(true)` |
| `read_acks(game_dir)` | Load our own Data.lua back and return its `acks`. No file → empty |

**Edge cases from the spec:**
- **Atomic writes.** The game must never read half a file, which is why it's `.tmp` then rename.
- **Only rewrite on change.** `render_data` must give byte-identical text for the same input, so that `write_if_changed` can tell. `Acks` is a `BTreeMap`, so its order is already stable.
- **This is the only code that writes into the game folder.** It never touches SavedVariables.

**You're done when:** `cargo test data_addon` passes (9 tests).

<details><summary>Stuck? interface_version</summary>

Same moves as step 1: `fs::read_to_string(main_toc_path(game_dir)).ok()?`, then `.lines()`, `trim()`, `strip_prefix("## Interface:")`, and `trim()` the rest. `find_map` returns the first line that matches. You'll need `use std::fs;` at the top.
</details>

<details><summary>Stuck? render_data</summary>

Easiest route: build one `serde_json::Value` with `version`, `generated_at`, `runs` and `acks`, then call your `lua::to_lua` on it, with `MurlocDefenceLeagueData = ` in front and a newline at the end.

- `serde_json::to_value(acks)` turns the acks map into a JSON object (`Ack` derives `Serialize`).
- `serde_json::Map` keeps keys in the order you insert them only with serde_json's `preserve_order` feature. Without it, keys come out sorted, which is also stable, so either is fine for "same input, same text".
</details>

<details><summary>Stuck? write_if_changed</summary>

- `fs::read_to_string(path).ok().as_deref() == Some(text)` is the "already the same?" check.
- `path.with_extension("lua.tmp")` makes `Data.lua.tmp`.
- `fs::write` then `fs::rename`. Rename over an existing file works on Windows and Linux.
</details>

<details><summary>Stuck? read_acks</summary>

Same as your `read_key`: run the file with `mlua`, get the `MurlocDefenceLeagueData` table, then get `acks` from it. To turn a Lua table into `Acks` in one go, use `mlua::LuaSerdeExt`: `lua.from_value(acks_value)`. The `serde` feature is already on.

Gotcha: an empty Lua table `{}` can't say whether it's a list or a map, so mlua reads it as an empty *list*, and deserializing that into a map fails. Check for an empty table first (`table.pairs::<mlua::Value, mlua::Value>().count() == 0`) and return `Acks::new()`.
</details>

---

## Step 8: `lfg.rs` helpers

The types are done (they mirror the spec's JSON). Four small functions:

| Function | What it does |
|---|---|
| `parse_outbox(json)` | The `lfgOutbox` string → `Outbox`. Bad JSON → `Err(message)` |
| `client_id(item)` | An outbox item's `client_id`, if it's a string |
| `pending(outbox, acks)` | Items still to send: has a `client_id`, isn't in `acks`, and isn't a repeat (keep the first). Order kept |
| `prune_acks(acks, now)` | Drop acks more than 7 days old. Exactly 7 days is kept |

**Why are runs and items `serde_json::Value` and not structs?** The spec says readers ignore fields they don't know, and the outbox goes to the site unchanged. A strict struct would quietly drop any field a newer addon or website adds. `Run` and `Signup` exist only as typed views for the dashboard.

**You're done when:** `cargo test lfg` passes (7 tests).

<details><summary>Stuck?</summary>

- `parse_outbox`: `serde_json::from_str(json).map_err(|e| e.to_string())`. That's the whole thing.
- `client_id`: `item.get("client_id")?.as_str()`.
- `pending`: `.iter().filter(...).cloned().collect()`. For "not a repeat", keep a `HashSet` of ids seen so far. `HashSet::insert` returns `false` if the id was already there, which is exactly the test you need.
- `prune_acks`: `BTreeMap::retain(|_, ack| now - ack.at <= ACK_KEEP_SECS)`.
</details>

---

## Step 9: `api.rs`: `get_lfg()` and `post_signups()`

Same shape as `me()` and `post_murlocs()`. The only new bits:

- **`get_lfg(etag)`:** if there's an etag, add the header `If-None-Match: <etag>`. A **304** means `Ok(LfgFetch::NotModified)` and there's no body to read. A **200** means read the `ETag` response header *before* `.json()` (which consumes the response), then `Ok(LfgFetch::Fresh { snapshot, etag })`.
- **`post_signups(items)`:** the body is `{"signups": items}`. The 200 body is `SignupResults { results }`; return `results`.
- **404 already becomes `ApiError::NotAvailable`.** That's how the app tells "the site has no group finder yet" apart from "the site is down".

When you're done, remove the `#[allow(dead_code)]` lines above `LfgFetch`, `SignupResults` and these two functions.

**You're done when:** with the mock running, a quick `#[test] #[ignore]` calling `get_lfg(None)` gets runs, and calling it again with the returned etag gets `NotModified`.

<details><summary>Stuck?</summary>

- Optional header: build the request into a variable, then `if let Some(tag) = etag { req = req.header(reqwest::header::IF_NONE_MATCH, tag); }`.
- Reading `ETag`: `res.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()).map(String::from)`. It's the same chain as `Retry-After`.
- Match on `res.status().as_u16()`: `304 => …`, `200 => …`, `_ => Err(Self::error_from(res))`.
</details>

---

## Step 10: `worker.rs`: `lfg_start()`, `poll_lfg()`, `relay_signups()`

The wiring is done:
- `start()` calls `lfg_start()` when the switch is on.
- The main loop calls `poll_lfg()` when `self.lfg.next_poll` is due.
- `check_file` calls `relay_signups(file, &text)` every time an account's file changed, before the murloc steps.
- `publish_lfg()` copies `self.lfg` into the dashboard's Group finder panel.

Each method has a numbered comment. The longer version:

### `lfg_start()`
1. `read_acks(game_dir)` into `self.lfg.acks`. **If Data.lua doesn't exist**, also set `config.lfg_etag = None` and save. Otherwise the first poll could get a 304 ("nothing changed") and there'd be no runs to write.
2. `interface_version(game_dir)`, or `FALLBACK_INTERFACE`, into `self.lfg.interface`.

`start()` has already worked out the game folder (it's in `Status.game_dir`). You may find it simplest to keep a copy on the worker, for example a `game_dir: Option<PathBuf>` field set in `start()`.

### `poll_lfg()`
| `get_lfg` result | Do this |
|---|---|
| `NotModified` | Set `last_poll`. Nothing else changed |
| `Fresh { snapshot, etag }` | Check `snapshot.version == 1` (if not, show "the website speaks a newer version; update the app" and stop). Store the snapshot, save the etag to config, prune acks, write the .toc and Data.lua |
| `NotAvailable` | Say once "the website doesn't have the group finder yet". Keep polling at the normal rate |
| `RateLimited(d)` | Next poll in `d` (at least `LFG_MIN_GAP`) |
| `Transient` | Show it. Next poll as normal |
| `Unauthorized` / `NotOfficer` | Paused, exactly as in `check_file` |

Always finish by setting `self.lfg.next_poll` and calling `self.publish_lfg()`.

### `relay_signups(file, text)`
1. `read_key(text, "lfgOutbox")`. `None` means nothing to do.
2. `parse_outbox`, then `pending(&outbox, &self.lfg.acks)`. Append to `self.lfg.pending` anything not already in it.
3. If anything is pending, call `post_signups`. On `Ok(results)`, add an `Ack::from_result(r, now)` per result, remove those from `pending`, count them, and log each rejection with its reason (the spec's table says what the addon will tell the player). On `Err`, keep `pending` (the site ignores repeats, so resending is safe) and handle the error as in `poll_lfg`.
4. Prune acks, rewrite Data.lua, `publish_lfg()`.

**The addon only clears an outbox item when Data.lua has its ack.** The player then has to `/reload` to pick up the new Data.lua. That's the spec's "nothing is instant".

**Now** is a Unix-seconds `i64`: `jiff::Timestamp::now().as_second()`.

**You're done when** (with the mock and the switch on):
- The Group finder panel shows the mock's two runs, and `Data.lua` appears in `Interface/AddOns/MurlocDefenceLeague_Data/` next to a `.toc` with the right Interface number.
- A second poll is a 304: `Data.lua` isn't rewritten (check its modified time).
- Hand-add an `lfgOutbox` line to a SavedVariables file (as in phase 1, while logged out), with one good sign-up and one from "Kat …". Both get acks in Data.lua, the Kat one shows `rejected` / `not_linked`, and the next poll's runs include the good one.
- Restart: the acks are still there (read back from Data.lua), and nothing is re-sent.

Then delete the `#![allow(dead_code)]` lines at the top of `lua.rs`, `data_addon.rs` and `lfg.rs`, and fix anything the compiler points out.

---

## Extras: the weekly leaderboard, the countdown, and the addon version

These three are wired into the dashboard already. Until you write them, each shows a "step N isn't written yet" line instead of crashing (the stubs return "not written" values, not `todo!()`). Their tests are waiting, as before.

## Step 11: `report.rs`: this week's murloc slayers

**Job:** read the export locally (the app still uploads it untouched), so the dashboard's **This week's murloc slayers** panel can show the top five.

| Function | What it does |
|---|---|
| `parse_export(text)` | Header `MDLMURLOCS;1;<exported_at>;<by>`, then `W;<week>;<name>;<kills>` and `T;<name>;<murloc>;<n>` lines. Anything else → `Err("line N: …")` |
| `latest_week(report)` | The biggest `week_start`, or `None` |
| `leaderboard(report, week)` | That week's rows: most kills first, ties by name, each with their `T` murloc |

**Edge cases:** names have spaces and accents but never `;`. A version other than 1 is an error (a newer format). Blank lines are fine. 0-kill rows stay (they played with the addon).

**You're done when:** `cargo test report` passes (9 tests) and the panel shows real names after your next `/reload`.

<details><summary>Stuck?</summary>

- `text.lines().enumerate()` gives `(index, line)`; line number = index + 1.
- `line.split(';').collect::<Vec<_>>()`, then match on the slice: `match parts.as_slice() { ["W", week, name, kills] => …, ["T", name, murloc, n] => …, _ => Err(…) }`. Slice patterns are made for this.
- `"156".parse::<u32>()` returns a `Result`; `.map_err(|_| format!("line {n}: …"))?` turns it into your error.
- Sorting by two keys: `rows.sort_by(|a, b| b.kills.cmp(&a.kills).then_with(|| a.character.cmp(&b.character)))`. Note `b` before `a` for "most first".
</details>

## Step 12: `schedule.rs`: the Wednesday clock

**Job:** two facts about the guild's week, both in the module's doc comment:
- the weekly **reset** is **Wednesday 04:00 UTC** (every `week_start` in real exports is exactly that);
- the website posts the Impact Report to Discord on **Wednesday at 19:00 UK time**, which is 19:00 UTC in winter and 18:00 UTC in summer (BST).

| Function | What it does |
|---|---|
| `last_reset(now)` | The latest Wednesday 04:00 UTC at or before `now` |
| `next_discord_post(now)` | The next Wednesday 19:00 Europe/London strictly after `now` |
| `nudge(now, exported_at)` | On a UK Wednesday, after the reset and before 19:00, if the newest export is older than the reset → `Some(NUDGE)` |

The panel title shows the countdown ("Discord post in 2d 4h"), and the nudge appears in yellow on Wednesdays, so officers log in and out after the reset and the closed week's figures make the post.

**You're done when:** `cargo test schedule` passes (11 tests). They cover the clocks changing in October, and the exact second of the post.

<details><summary>Stuck? Time zones with jiff</summary>

- A moment in a zone: `now.in_tz("Europe/London")?` gives a `ZonedDateTime`; `now.to_zoned_datetime(jiff::tz::TimeZone::UTC)` for UTC.
- Same day, different time: `zdt.with().hour(19).minute(0).second(0).subsec_nanosecond(0).build()?`. jiff works out GMT versus BST for you.
- Step a day at a time: `use jiff::ToSpan;` then `zdt.checked_add(1.day())?`. A loop that walks forwards (or backwards for the reset) until `weekday()` is Wednesday and the time is on the right side of `now` is simple and correct.
- `zdt.timestamp()` turns it back into a `Timestamp`; `.as_second()` gives Unix seconds.
- In `nudge`, "after the reset" means `last_reset(now)` falls on the same UTC date as `now`.
</details>

## Step 13: is the guild addon up to date?

**Job:** show the installed addon version in the header, and whether the website says there's a newer one. The real payoff is the case you hit: when an account has no `murlocExport` *because* the addon is too old, the warning says "update the addon to 1.3.0 or later".

The website doesn't have this yet. The **proposed** endpoint (the mock already serves it) is:

```
GET /api/v1/addon  →  { "latest": "1.4.0", "min_for_murlocs": "1.3.0", "url": "https://…" }
```

`min_for_murlocs` is the first addon version that writes `murlocExport`. If the website ends up shaped differently, change `AddonInfo` in `addon.rs` to match.

You write three things:

| Where | Function | What it does |
|---|---|---|
| `game.rs` | `installed_addon_version(game_dir)` | The `## Version:` line from the main addon's `.toc`, the same move as `interface_version` in step 7 |
| `api.rs` | `addon_info()` | `GET /addon`, the same pattern as `me()`. Until the site has it, the 404 becomes `NotAvailable` and the app stays quiet |
| `addon.rs` | `addon_status(installed, info)` | Compare with `Version` (`version.rs`, done): `update_available`, `too_old_for_murlocs`, and pass `url` through |

**You're done when:** `cargo test addon` passes (6 tests), and with the mock running and an addon `.toc` saying `## Version: 1.2.0`, the header shows "too old to send figures: needs 1.3.0+".

<details><summary>Stuck?</summary>

- `Version::parse("1.3.0")` returns an `Option<Version>`; versions compare with `<` / `>` or `.is_newer_than(&other)`.
- `Option` chains keep this short: `info.and_then(|i| Version::parse(&i.latest))`.
- Matching two options at once: `match (installed, min) { (Some(i), Some(m)) if m.is_newer_than(&i) => Some(m), _ => None }`.
</details>

