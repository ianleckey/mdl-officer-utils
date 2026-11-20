//! Phase 2: the group finder's data, exactly as the spec shapes it.
//!
//! Two directions:
//! * Website → game: `GET /lfg` gives an `LfgSnapshot`, which goes into Data.lua.
//! * Game → website: the addon's `lfgOutbox` (an `Outbox`) goes to
//!   `POST /lfg/signups`, and each `SignupResult` becomes an `Ack` in Data.lua.
//!
//! Runs and outbox items stay as `serde_json::Value`, so any field the website
//! or addon adds later is passed on untouched (the spec: "readers ignore fields
//! they don't know"). `Run` and `Signup` are typed *views* of them, used only
//! where the app itself needs a field (the dashboard).
//!
//! The types are finished. The four functions at the bottom are GUIDE.md step 8.

// Phase 2 skeleton: quiets "never used" warnings until step 10 wires this in.
// Delete this line then, and let the compiler point out anything left over.
#![allow(dead_code)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long acks stay in Data.lua (the spec: 7 days).
pub const ACK_KEEP_SECS: i64 = 7 * 24 * 60 * 60;

/// `GET /lfg` → 200.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LfgSnapshot {
    pub version: u32,
    pub generated_at: i64,
    /// Each run exactly as the website sent it. See `Run` for the fields.
    pub runs: Vec<Value>,
}

/// A typed view of one run, for the dashboard. Fields the app doesn't show are left out.
#[derive(Debug, Clone, Deserialize)]
pub struct Run {
    pub id: String,
    /// "dungeon" or "battleground".
    pub kind: String,
    pub title: String,
    /// Unix seconds.
    pub starts_at: i64,
    pub size: u32,
    #[serde(default)]
    pub demo: bool,
    #[serde(default)]
    pub signups: Vec<Signup>,
}

impl Run {
    /// Reads a run's typed view; `None` if the website sent something odd.
    pub fn from_value(v: &Value) -> Option<Run> {
        serde_json::from_value(v.clone()).ok()
    }

    /// How many have a place ("in"), as the run page counts them.
    pub fn filled(&self) -> usize {
        self.signups.iter().filter(|s| s.status == "in").count()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Signup {
    pub character: String,
    /// "tank", "healer", "melee" or "ranged".
    pub role: String,
    /// "in", "waiting" or "out".
    pub status: String,
}

/// The addon's `lfgOutbox`, once parsed: `{ "version": 1, "signups": [ ... ] }`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Outbox {
    pub version: u32,
    /// Each item exactly as `POST /lfg/signups` takes it (has a `client_id`).
    pub signups: Vec<Value>,
}

/// One entry of `POST /lfg/signups` → 200 `results`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SignupResult {
    pub client_id: String,
    /// "ok" or "rejected".
    pub status: String,
    /// Set when rejected: unknown_run, run_closed, not_linked, not_in_guild, invalid, mains_only.
    #[serde(default)]
    pub reason: Option<String>,
}

/// The website's answer to one outbox item, kept in Data.lua's `acks` for 7 days.
/// The addon drops an outbox item once its `client_id` has an ack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ack {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When the app got the answer, Unix seconds.
    pub at: i64,
}

impl Ack {
    pub fn from_result(r: &SignupResult, now: i64) -> Ack {
        Ack { status: r.status.clone(), reason: r.reason.clone(), at: now }
    }
}

/// `client_id` → its ack. A `BTreeMap`, so Data.lua lists them in a stable order
/// (and an unchanged set of acks writes byte-identical text).
pub type Acks = BTreeMap<String, Ack>;

// ---------------------------------------------------------------------------
// Yours: GUIDE.md step 8. Tests below.
// ---------------------------------------------------------------------------

/// Parses the `lfgOutbox` string (already un-escaped by `savedvars::read_key`).
/// Bad JSON is an `Err` with a message fit for the Events panel.
pub fn parse_outbox(json: &str) -> Result<Outbox, String> {
    let _ = json;
    todo!("step 8 in GUIDE.md")
}

/// The `client_id` of an outbox item, if it has a string one.
pub fn client_id(item: &Value) -> Option<&str> {
    let _ = item;
    todo!("step 8 in GUIDE.md")
}

/// The outbox items that still need sending: drop any without a `client_id`, any
/// already in `acks`, and repeats of the same `client_id` (keep the first).
/// Order is kept.
pub fn pending(outbox: &Outbox, acks: &Acks) -> Vec<Value> {
    let _ = (outbox, acks);
    todo!("step 8 in GUIDE.md")
}

/// Removes acks older than 7 days (`ACK_KEEP_SECS`) before `now` (Unix seconds).
/// Exactly 7 days old is still kept.
pub fn prune_acks(acks: &mut Acks, now: i64) {
    let _ = (acks, now);
    todo!("step 8 in GUIDE.md")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ack(at: i64) -> Ack {
        Ack { status: "ok".into(), reason: None, at }
    }

    #[test]
    fn parses_the_spec_outbox() {
        let o = parse_outbox(
            r#"{"version":1,"signups":[{"client_id":"Shoop-1795219200-3f2a","run_id":"r1","character":"Shoop Dahoop",
                "role":"tank","spec":"Protection","availability":"coming","note":"","signed_at":1795219200,
                "relayed_by":"Reverend Drew","some_future_field":true}]}"#,
        )
        .unwrap();
        assert_eq!(o.version, 1);
        assert_eq!(o.signups.len(), 1);
        // Unknown fields survive, so they reach the website unchanged.
        assert_eq!(o.signups[0]["some_future_field"], json!(true));
    }

    #[test]
    fn empty_outbox() {
        assert_eq!(parse_outbox(r#"{"version":1,"signups":[]}"#).unwrap().signups.len(), 0);
    }

    #[test]
    fn bad_outbox_is_an_error() {
        assert!(parse_outbox("{not json").is_err());
        assert!(parse_outbox(r#"{"version":1}"#).is_err()); // signups missing
    }

    #[test]
    fn client_ids() {
        assert_eq!(client_id(&json!({"client_id": "Kat-1-9b1c"})), Some("Kat-1-9b1c"));
        assert_eq!(client_id(&json!({"client_id": 5})), None);
        assert_eq!(client_id(&json!({})), None);
    }

    #[test]
    fn pending_drops_acked_repeated_and_unnamed() {
        let outbox = Outbox {
            version: 1,
            signups: vec![
                json!({"client_id": "A", "n": 1}),
                json!({"client_id": "B"}),
                json!({"client_id": "A", "n": 2}), // repeat: keep the first
                json!({"no_id": true}),
                json!({"client_id": "C"}),
            ],
        };
        let acks = Acks::from([("B".to_string(), ack(0))]);
        let left = pending(&outbox, &acks);
        assert_eq!(left, vec![json!({"client_id": "A", "n": 1}), json!({"client_id": "C"})]);
    }

    #[test]
    fn prune_keeps_a_week() {
        let now = 1_795_219_200;
        let mut acks = Acks::from([
            ("fresh".to_string(), ack(now - 60)),
            ("exactly-a-week".to_string(), ack(now - ACK_KEEP_SECS)),
            ("stale".to_string(), ack(now - ACK_KEEP_SECS - 1)),
        ]);
        prune_acks(&mut acks, now);
        let left: Vec<_> = acks.keys().map(String::as_str).collect();
        assert_eq!(left, vec!["exactly-a-week", "fresh"]);
    }

    #[test]
    fn run_view_reads_the_spec_example() {
        let run = Run::from_value(&json!({
            "id": "6fda58ce", "kind": "dungeon", "title": "The Stockade", "instance": "The Stockade",
            "starts_at": 1795230000, "duration_minutes": 90, "size": 5,
            "slots": {"tank": 1, "healer": 1, "dps": 3}, "leader": "Gotrek Rustpeak", "notes": "",
            "demo": false, "url": "https://example/lfg/6fda58ce",
            "signups": [{"character": "Gotrek Rustpeak", "class": "Warrior", "role": "tank",
                         "spec": "Protection", "availability": "coming", "status": "in", "note": ""}]
        }))
        .unwrap();
        assert_eq!(run.title, "The Stockade");
        assert_eq!(run.filled(), 1);
    }
}
