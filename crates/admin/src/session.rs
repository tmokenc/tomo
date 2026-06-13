//! Signed-cookie sessions.
//!
//! The session cookie carries a small JSON payload (`user_id`, `username`,
//! `expires_at`) and is signed by `axum-extra`'s `SignedCookieJar` using the
//! key derived from `TOMO_ADMIN_SESSION_SECRET`. We set:
//! * `HttpOnly`           — JS cannot read it (defence against XSS).
//! * `Secure`             — only sent over HTTPS (off only when binding to localhost http).
//! * `SameSite=Lax`       — blocks CSRF on cross-site top-level loads.
//! * Short TTL            — re-login required after `session_ttl`.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use axum_extra::extract::cookie::{Cookie, SameSite};

pub const COOKIE_NAME: &str = "tomo_session";
pub const STATE_COOKIE: &str = "tomo_oauth_state";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub user_id: u64,
    pub username: String,
    pub expires_at: DateTime<Utc>,
}

impl Session {
    pub fn new(user_id: u64, username: String, ttl: Duration) -> Self {
        Self {
            user_id,
            username,
            expires_at: Utc::now() + ttl,
        }
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at
    }

    pub fn to_cookie<'a>(&self, secure: bool) -> Cookie<'a> {
        let body = serde_json::to_string(self).unwrap_or_default();
        let mut cookie = Cookie::new(COOKIE_NAME, body);
        cookie.set_http_only(true);
        cookie.set_secure(secure);
        cookie.set_same_site(SameSite::Lax);
        cookie.set_path("/");
        cookie
    }
}

pub fn clear_cookie<'a>(name: &'a str) -> Cookie<'a> {
    let mut cookie = Cookie::new(name, "");
    // A removal cookie's path must match the path it was *set* with, or the
    // browser keeps the original. The state cookie is scoped to `/oauth`
    // (see `OauthFlow::to_cookie`); everything else uses `/`.
    cookie.set_path(if name == STATE_COOKIE { "/oauth" } else { "/" });
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_max_age(time::Duration::seconds(0));
    cookie
}

/// Short-lived (10 min) cookie carrying the OAuth `state` token & PKCE
/// verifier so we can validate them on the redirect back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OauthFlow {
    pub csrf_token: String,
    pub pkce_verifier: String,
    pub expires_at: DateTime<Utc>,
}

impl OauthFlow {
    pub fn new(csrf: String, pkce: String) -> Self {
        Self {
            csrf_token: csrf,
            pkce_verifier: pkce,
            expires_at: Utc::now() + Duration::minutes(10),
        }
    }

    pub fn to_cookie<'a>(&self, secure: bool) -> Cookie<'a> {
        let body = serde_json::to_string(self).unwrap_or_default();
        let mut cookie = Cookie::new(STATE_COOKIE, body);
        cookie.set_http_only(true);
        cookie.set_secure(secure);
        cookie.set_same_site(SameSite::Lax);
        cookie.set_path("/oauth");
        cookie.set_max_age(time::Duration::minutes(10));
        cookie
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at
    }
}

