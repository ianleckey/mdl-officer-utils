//! Talking to the website. See GUIDE.md, step 3.
//!
//! Base URL: `Config::api_base` (normally https://murlocdefenseleague.org/api/v1).
//! JSON in and out. Every request sends `Authorization: Bearer <token>`.

use std::time::Duration;
use reqwest::blocking::Response;
use serde::Deserialize;

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
#[allow(dead_code)] // Phase 2 skeleton: remove once step 10 uses it.
#[derive(Debug, Clone)]
pub enum LfgFetch {
    /// 304: nothing has changed since the ETag we sent.
    NotModified,
    /// 200: the current runs, and the ETag to send next time.
    Fresh { snapshot: LfgSnapshot, etag: Option<String> },
}

/// The body of a 200 from `POST /lfg/signups`.
#[allow(dead_code)] // Phase 2 skeleton: remove once step 10 uses it.
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

pub struct Client {
    http: reqwest::blocking::Client,
    base: String,
    token: String,
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
        }
    }

    fn error_from(response: Response) -> ApiError {
        let status = response.status().as_u16();
        match status {
            401 => ApiError::Unauthorized,
            403 => ApiError::NotOfficer,
            422 => match response.json::<ErrorBody>() {
                Ok(body) => ApiError::Rejected(body.error),
                Err(_) => ApiError::Rejected("the website rejected it".to_string()),
            },
            429 => {
                let secs = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)     
                    .and_then(|v| v.to_str().ok())         
                    .and_then(|s| s.parse::<u64>().ok())   
                    .unwrap_or(60);                        
                ApiError::RateLimited(Duration::from_secs(secs))
            }
            404 => ApiError::NotAvailable,
            other => ApiError::Transient(format!("the website answered {other}")),
        }
    }

    /// Joins the base URL and a path: `url("/me")`.
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// `GET /me`: checks the token.
    pub fn me(&self) -> Result<Me, ApiError> {
        let res = self.http.get(self.url("/me"))
                    .bearer_auth(&self.token)
                    .send()
                    .map_err(|e| ApiError::Transient(e.to_string()))?;
        if res.status().as_u16() == 200 {
            res.json::<Me>().map_err(|e| ApiError::Transient(e.to_string()))
        } else {
            Err(Self::error_from(res))
        }
    }

    /// `POST /murlocs` with `{"export": export}`, the text passed on exactly as read.
    pub fn post_murlocs(&self, export: &str) -> Result<ImportResult, ApiError> {
        let value = serde_json::json!({ "export": export }); 
        let res = self.http.post(self.url("/murlocs"))
                    .bearer_auth(&self.token)
                    .json(&value)
                    .send()
                    .map_err(|e| ApiError::Transient(e.to_string()))?;

        if res.status().as_u16() == 200 {
            res.json::<ImportResult>().map_err(|e| ApiError::Transient(e.to_string()))
        } else {
            Err(Self::error_from(res))
        }
    }

    /// `GET /addon`: the latest guild addon version (GUIDE.md step 13). The
    /// website doesn't have this endpoint yet; `addon.rs` has the proposed shape.
    ///
    /// Yours. It's called on every start-up, so until it's written it answers
    /// `NotAvailable`, exactly what the real site's 404 will give today.
    pub fn addon_info(&self) -> Result<AddonInfo, ApiError> {
        Err(ApiError::NotAvailable)
    }

    // -----------------------------------------------------------------------
    // Phase 2: the group finder. GUIDE.md step 9.
    // -----------------------------------------------------------------------

    /// `GET /lfg`, sending `If-None-Match: <etag>` when we have one.
    ///
    /// * 304 → `Ok(LfgFetch::NotModified)` (no body)
    /// * 200 → `Ok(LfgFetch::Fresh { snapshot, etag })`, with `etag` from the response's
    ///   `ETag` header (`None` if the site didn't send one)
    /// * anything else → `Err(Self::error_from(res))`, as in `me()`
    #[allow(dead_code)] // Phase 2 skeleton: remove once step 10 uses it.
    pub fn get_lfg(&self, etag: Option<&str>) -> Result<LfgFetch, ApiError> {
        let _ = etag;
        todo!("step 9 in GUIDE.md")
    }

    /// `POST /lfg/signups` with `{"signups": [...]}`, the outbox items passed on
    /// unchanged. 200 → the `results` list, one per item.
    #[allow(dead_code)] // Phase 2 skeleton: remove once step 10 uses it.
    pub fn post_signups(&self, signups: &[serde_json::Value]) -> Result<Vec<SignupResult>, ApiError> {
        let _ = signups;
        todo!("step 9 in GUIDE.md")
    }
}
