#!/usr/bin/env python3
"""A fake Murloc Defence League website, for trying the desktop app before the
real endpoints exist. Development only; standard library only.

    python3 tools/mock_site.py            # listens on http://127.0.0.1:8787
    python3 tools/mock_site.py --port 9000

Then, in the app's config.toml (Linux: ~/.config/mdl-desktop/config.toml):

    api_base = "http://127.0.0.1:8787/api/v1"
    token = "test"
    lfg_enabled = true

It speaks the spec's API v1:
  GET  /api/v1/me             → {"name": "Mock Officer", "officer": true}
  POST /api/v1/murlocs        → an ImportResult (changed is true the first time a text is seen)
  GET  /api/v1/lfg            → runs, with a real ETag; 304 when If-None-Match matches
  POST /api/v1/lfg/signups    → one result per item; idempotent by client_id
  GET  /api/v1/addon          → {"latest": "1.4.0", "min_for_murlocs": "1.3.0", "url": ...}
                                (proposed endpoint for the addon version check)

Sign-ups are rejected to match the spec's reasons, so each one can be tested:
  run_id "nope"          → unknown_run
  character "Kat ..."    → not_linked
  character "Rando ..."  → not_in_guild
  role "pilot"           → invalid
Everything else is "ok" and is added to that run's signups (so the next /lfg changes).

Handy extras:
  POST /mock/token-revoked   → every request now gets 401 (restart the mock to undo)
  POST /mock/down            → every request now gets 503
  POST /mock/up              → back to normal
  POST /mock/no-lfg          → /lfg and /lfg/signups answer 404 (the real site today)
  POST /mock/no-addon-api    → /addon answers 404 (the real site today)
"""

import argparse
import hashlib
import json
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

TOKEN = "test"
NOW = int(time.time())

# The spec's example run, plus a battleground, starting soon.
RUNS = [
    {
        "id": "6fda58ce-9c55-4fc1-9a2d-be31314acebb",
        "kind": "dungeon",
        "title": "The Stockade",
        "instance": "The Stockade",
        "starts_at": NOW + 3 * 3600,
        "duration_minutes": 90,
        "size": 5,
        "slots": {"tank": 1, "healer": 1, "dps": 3},
        "leader": "Gotrek Rustpeak",
        "notes": "Meet at the Stockade entrance.",
        "demo": False,
        "url": "https://murlocdefenseleague.org/lfg/6fda58ce-9c55-4fc1-9a2d-be31314acebb",
        "signups": [
            {"character": "Gotrek Rustpeak", "class": "Warrior", "role": "tank", "spec": "Protection",
             "availability": "coming", "status": "in", "note": ""}
        ],
    },
    {
        "id": "0b6d1e52-3c1e-4a8e-9f0d-7a5b2c9e1f44",
        "kind": "battleground",
        "title": "Warsong Gulch",
        "instance": "Warsong Gulch",
        "starts_at": NOW + 26 * 3600,
        "duration_minutes": 60,
        "size": 10,
        "slots": {"tank": 0, "healer": 0, "dps": 10},
        "leader": "Reverend Drew",
        "notes": "",
        "demo": True,
        "url": "https://murlocdefenseleague.org/lfg/0b6d1e52-3c1e-4a8e-9f0d-7a5b2c9e1f44",
        "signups": [],
    },
]

state = {"mode": "up", "addon_api": True, "seen_exports": set(), "answered": {}}

ADDON = {"latest": "1.4.0", "min_for_murlocs": "1.3.0", "url": "https://murlocdefenseleague.org/addon"}


def lfg_body():
    return {"version": 1, "generated_at": int(time.time()), "runs": RUNS}


def etag_of(body):
    runs_only = json.dumps(body["runs"], sort_keys=True).encode()
    return '"' + hashlib.sha256(runs_only).hexdigest()[:16] + '"'


def answer_signup(item):
    cid = item.get("client_id")
    if cid in state["answered"]:
        return state["answered"][cid]  # idempotent: same answer, no change
    run = next((r for r in RUNS if r["id"] == item.get("run_id")), None)
    character = str(item.get("character", ""))
    if run is None:
        result = {"client_id": cid, "status": "rejected", "reason": "unknown_run"}
    elif character.startswith("Kat"):
        result = {"client_id": cid, "status": "rejected", "reason": "not_linked"}
    elif character.startswith("Rando"):
        result = {"client_id": cid, "status": "rejected", "reason": "not_in_guild"}
    elif item.get("role") not in ("tank", "healer", "melee", "ranged"):
        result = {"client_id": cid, "status": "rejected", "reason": "invalid"}
    else:
        result = {"client_id": cid, "status": "ok"}
        run["signups"] = [s for s in run["signups"] if s["character"] != character]
        if item.get("availability") != "unavailable":
            run["signups"].append({
                "character": character, "class": "Unknown", "role": item.get("role"),
                "spec": item.get("spec", ""), "availability": item.get("availability", "coming"),
                "status": "in", "note": item.get("note", ""),
            })
    state["answered"][cid] = result
    return result


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        print(time.strftime("%H:%M:%S"), self.command, self.path, "→", args[1] if len(args) > 1 else "")

    def send_json(self, code, body=None, headers=None):
        data = b"" if body is None else json.dumps(body).encode()
        self.send_response(code)
        if body is not None:
            self.send_header("Content-Type", "application/json")
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def read_json(self):
        n = int(self.headers.get("Content-Length", 0))
        try:
            return json.loads(self.rfile.read(n) or b"null")
        except json.JSONDecodeError:
            return None

    def gate(self):
        """Common checks. Returns True if a response was already sent."""
        if state["mode"] == "down":
            self.send_json(503, {"error": "mock: down"})
            return True
        if state["mode"] == "revoked" or self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self.send_json(401, {"error": "unknown token"})
            return True
        return False

    def do_GET(self):
        if self.gate():
            return
        if self.path == "/api/v1/me":
            return self.send_json(200, {"name": "Mock Officer", "officer": True})
        if self.path == "/api/v1/addon":
            if not state["addon_api"]:
                return self.send_json(404, {"error": "not found"})
            return self.send_json(200, ADDON)
        if self.path == "/api/v1/lfg":
            if state["mode"] == "no-lfg":
                return self.send_json(404, {"error": "not found"})
            body = lfg_body()
            tag = etag_of(body)
            if self.headers.get("If-None-Match") == tag:
                return self.send_json(304, None, {"ETag": tag})
            return self.send_json(200, body, {"ETag": tag})
        self.send_json(404, {"error": "not found"})

    def do_POST(self):
        if self.path == "/mock/no-addon-api":
            state["addon_api"] = False
            return self.send_json(200, {"addon_api": False})
        if self.path.startswith("/mock/"):
            state["mode"] = {"token-revoked": "revoked", "down": "down", "up": "up", "no-lfg": "no-lfg"}.get(
                self.path[len("/mock/"):], state["mode"])
            return self.send_json(200, {"mode": state["mode"]})
        if self.gate():
            return
        body = self.read_json()
        if self.path == "/api/v1/murlocs":
            if not isinstance(body, dict) or not isinstance(body.get("export"), str):
                return self.send_json(422, {"error": "Body must be {\"export\": \"...\"}."})
            text = body["export"]
            if not text.startswith("MDLMURLOCS;1;"):
                return self.send_json(422, {"error": "That isn't an MDLMURLOCS;1 export."})
            lines = text.split("\n")
            header = lines[0].split(";")
            weeks = {l.split(";")[1] for l in lines if l.startswith("W;")}
            people = {l.split(";")[2] for l in lines if l.startswith("W;")}
            changed = text not in state["seen_exports"]
            state["seen_exports"].add(text)
            return self.send_json(200, {
                "changed": changed, "exported_at": int(header[2]) if len(header) > 2 and header[2].isdigit() else 0,
                "by": header[3] if len(header) > 3 else "", "weeks": len(weeks), "people": len(people),
                "tops": sum(1 for l in lines if l.startswith("T;")), "unmatched": [],
            })
        if self.path == "/api/v1/lfg/signups":
            if state["mode"] == "no-lfg":
                return self.send_json(404, {"error": "not found"})
            if not isinstance(body, dict) or not isinstance(body.get("signups"), list):
                return self.send_json(422, {"error": "Body must be {\"signups\": [...]}."})
            return self.send_json(200, {"results": [answer_signup(i) for i in body["signups"]]})
        self.send_json(404, {"error": "not found"})


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--port", type=int, default=8787)
    args = p.parse_args()
    print(f"Mock MDL website on http://127.0.0.1:{args.port}/api/v1 (token: {TOKEN!r})")
    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
