//! Talking to the website.
//!
//! Base URL: `Config::api_base` (normally https://murlocdefenseleague.org/api/v1).
//! JSON in and out. Every request sends `Authorization: Bearer <token>`.

use std::cell::Cell;
use std::time::{Duration, Instant, SystemTime};

use reqwest::StatusCode;
use reqwest::blocking::{RequestBuilder, Response};
use reqwest::header;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::addon::AddonInfo;
use crate::lfg::{LfgSnapshot, SignupResult};

/// The body of a 422: `{"error": "..."}`.
#[derive(Debug, Deserialize)]
struct ErrorBody {
    error: String,
}

/// `GET /me`
#[derive(Debug, Clone, Deserialize)]
pub struct Me {
    pub name: String,
    pub officer: bool,
}

/// `POST /murlocs` → 200
#[derive(Debug, Clone, Deserialize)]
pub struct ImportResult {
    /// false means the site already had these figures (or higher ones).
    pub changed: bool,
    pub exported_at: i64,
    pub by: String,
    pub weeks: u32,
    pub people: u32,
    pub tops: u32,
    /// Names the site couldn't match to a guild character. Skipped, not an error.
    #[serde(default)]
    pub unmatched: Vec<String>,
}

/// What `GET /lfg` can come back with.
#[derive(Debug, Clone)]
pub enum LfgFetch {
    /// 304: nothing has changed since the ETag we sent.
    NotModified,
    /// 200: the current runs, and the ETag to send next time.
    Fresh { snapshot: LfgSnapshot, etag: Option<String> },
}

/// The body of a 200 from `POST /lfg/signups`.
#[derive(Debug, Clone, Deserialize)]
pub struct SignupResults {
    pub results: Vec<SignupResult>,
}

/// Everything that can go wrong, sorted by what the worker should do about it.
#[derive(Debug, Clone)]
pub enum ApiError {
    /// 401: missing or unknown token. Stop until the officer fixes the settings.
    Unauthorized,
    /// 403: the token's owner isn't an officer any more. Stop until the settings change.
    NotOfficer,
    /// 422: the site says the body is wrong. The string is `{"error": "..."}` from the
    /// body, a sentence fit to show the officer.
    Rejected(String),
    /// 429: too many requests. Wait this long (from the `Retry-After` header).
    RateLimited(Duration),
    /// 5xx, network down, timeouts, a response we couldn't read. Back off and try again.
    Transient(String),
    /// 404: the website doesn't have this endpoint (yet). Phase 2 uses this to tell
    /// "the group finder isn't live on the site" apart from "the site is down".
    NotAvailable,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Unauthorized => write!(f, "The website doesn't recognise this token."),
            ApiError::NotOfficer => write!(f, "This token's owner isn't an officer any more."),
            ApiError::Rejected(msg) => write!(f, "The website said: {msg}"),
            ApiError::RateLimited(d) => write!(f, "Too many requests; waiting {}s.", d.as_secs()),
            ApiError::Transient(msg) => write!(f, "Couldn't reach the website: {msg}"),
            ApiError::NotAvailable => write!(f, "The website doesn't have this feature yet."),
        }
    }
}

/// The outcome of the latest request, for the dashboard's website health line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    pub at: SystemTime,
    /// How long the request took (until the response headers, or the failure).
    pub took: Duration,
    /// True if the website gave a proper HTTP answer. False if it couldn't be
    /// reached at all, timed out, or answered with a 5xx.
    pub answered: bool,
}

pub struct Client {
    http: reqwest::blocking::Client,
    base: String,
    token: String,
    last_contact: Cell<Option<Contact>>,
}

impl Client {
    pub fn new(base: &str, token: &str) -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("mdl-desktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("building the HTTP client");
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            last_contact: Cell::new(None),
        }
    }

    /// How the latest request went, if there's been one.
    pub fn last_contact(&self) -> Option<Contact> {
        self.last_contact.get()
    }

    /// Sends a request with the token, noting how it went in `last_contact`.
    /// A network failure becomes `Transient`; any HTTP answer is returned as is.
    fn execute(&self, req: RequestBuilder) -> Result<Response, ApiError> {
        let started = Instant::now();
        let result = req.bearer_auth(&self.token).send();
        let answered = result.as_ref().is_ok_and(|res| !res.status().is_server_error());
        self.last_contact.set(Some(Contact { at: SystemTime::now(), took: started.elapsed(), answered }));
        result.map_err(|e| ApiError::Transient(e.to_string()))
    }

    /// Turns a response that wasn't the expected success into the matching `ApiError`.
    fn error_from(response: Response) -> ApiError {
        match response.status().as_u16() {
            401 => ApiError::Unauthorized,
            403 => ApiError::NotOfficer,
            422 => match response.json::<ErrorBody>() {
                Ok(body) => ApiError::Rejected(body.error),
                Err(_) => ApiError::Rejected("the website rejected it".to_string()),
            },
            429 => {
                let secs = response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(60);
                ApiError::RateLimited(Duration::from_secs(secs))
            }
            404 => ApiError::NotAvailable,
            // 5xx, and anything else unexpected (a 3xx we didn't ask for, say).
            other => ApiError::Transient(format!("the website answered {other}")),
        }
    }

    /// Joins the base URL and a path: `url("/me")`.
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Sends a request (see `execute`), and reads a 200's JSON body as `T`.
    /// Anything else becomes the matching `ApiError` (see `error_from`).
    fn send<T: DeserializeOwned>(&self, req: RequestBuilder) -> Result<T, ApiError> {
        let res = self.execute(req)?;
        if res.status() != StatusCode::OK {
            return Err(Self::error_from(res));
        }
        res.json::<T>().map_err(|e| ApiError::Transient(e.to_string()))
    }

    /// `GET /me`: checks the token.
    pub fn me(&self) -> Result<Me, ApiError> {
        self.send(self.http.get(self.url("/me")))
    }

    /// `POST /murlocs` with `{"export": export}`, the text passed on exactly as read.
    pub fn post_murlocs(&self, export: &str) -> Result<ImportResult, ApiError> {
        self.send(self.http.post(self.url("/murlocs")).json(&json!({ "export": export })))
    }

    /// `GET /addon`: the latest guild addon version. The website doesn't have
    /// this endpoint yet (its 404 becomes `NotAvailable`); `addon.rs` has the
    /// proposed shape.
    pub fn addon_info(&self) -> Result<AddonInfo, ApiError> {
        self.send(self.http.get(self.url("/addon")))
    }

    // -----------------------------------------------------------------------
    // Phase 2: the group finder.
    // -----------------------------------------------------------------------

    /// `GET /lfg`, sending `If-None-Match: <etag>` when we have one.
    ///
    /// * 304 → `Ok(LfgFetch::NotModified)` (no body)
    /// * 200 → `Ok(LfgFetch::Fresh { snapshot, etag })`, with `etag` from the response's
    ///   `ETag` header (`None` if the site didn't send one)
    /// * anything else → the matching `ApiError`
    pub fn get_lfg(&self, etag: Option<&str>) -> Result<LfgFetch, ApiError> {
        let mut req = self.http.get(self.url("/lfg"));
        if let Some(tag) = etag {
            req = req.header(header::IF_NONE_MATCH, tag);
        }
        let res = self.execute(req)?;
        match res.status() {
            StatusCode::NOT_MODIFIED => Ok(LfgFetch::NotModified),
            StatusCode::OK => {
                // Read the header first: `.json()` consumes the response.
                let etag = res.headers().get(header::ETAG).and_then(|v| v.to_str().ok()).map(String::from);
                let snapshot = res.json::<LfgSnapshot>().map_err(|e| ApiError::Transient(e.to_string()))?;
                Ok(LfgFetch::Fresh { snapshot, etag })
            }
            _ => Err(Self::error_from(res)),
        }
    }

    /// `POST /lfg/signups` with `{"signups": [...]}`, the outbox items passed on
    /// unchanged. 200 → the `results` list, one per item.
    pub fn post_signups(&self, signups: &[serde_json::Value]) -> Result<Vec<SignupResult>, ApiError> {
        let req = self.http.post(self.url("/lfg/signups")).json(&json!({ "signups": signups }));
        self.send::<SignupResults>(req).map(|r| r.results)
    }
}

/// Against `tools/mock_site.py`: start it first, then
/// `cargo test api::mock -- --ignored`.
#[cfg(test)]
mod mock {
    use super::*;

    fn client() -> Client {
        Client::new("http://127.0.0.1:8787/api/v1", "test")
    }

    #[test]
    #[ignore = "needs tools/mock_site.py running"]
    fn every_endpoint() {
        let c = client();
        assert!(c.me().unwrap().officer);
        assert_eq!(c.addon_info().unwrap().min_for_murlocs.as_deref(), Some("1.3.0"));
        let result = c.post_murlocs("MDLMURLOCS;1;1795219200;Mock\nW;1794369600;Shoop Dahoop;3").unwrap();
        assert_eq!(result.exported_at, 1795219200);

        let LfgFetch::Fresh { snapshot, etag } = c.get_lfg(None).unwrap() else { panic!("expected a 200") };
        assert!(!snapshot.runs.is_empty());
        let etag = etag.expect("the mock sends an ETag");
        assert!(matches!(c.get_lfg(Some(&etag)).unwrap(), LfgFetch::NotModified));
        assert!(c.last_contact().unwrap().answered, "a 304 is an answer");

        // A 5xx counts as no answer, for the dashboard's health line.
        let mock = |path: &str| c.http.post(format!("http://127.0.0.1:8787{path}")).send().unwrap();
        mock("/mock/down");
        assert!(matches!(c.me(), Err(ApiError::Transient(_))));
        assert!(!c.last_contact().unwrap().answered);
        mock("/mock/up");
        assert!(c.me().is_ok() && c.last_contact().unwrap().answered);
    }
}
