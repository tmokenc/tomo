//! Discord OAuth handlers.
//!
//! Flow:
//! 1. GET `/login` — generates `state` + PKCE verifier, drops them into a
//!    short-lived signed cookie, redirects to Discord's authorise URL.
//! 2. GET `/oauth/callback?code=…&state=…` — validates `state` against the
//!    cookie, exchanges `code` for an access token, fetches `/users/@me`,
//!    checks the user id against the bot's owner list (via gRPC), and on
//!    success sets the session cookie and redirects to `/`.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::SignedCookieJar;
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use serde::Deserialize;
use tonic::Request;
use tracing::{info, warn};

use tomo_rpc::CheckOwnerRequest;

use crate::session::{clear_cookie, OauthFlow, Session, COOKIE_NAME, STATE_COOKIE};
use crate::state::AppState;

const AUTHORIZE_URL: &str = "https://discord.com/api/oauth2/authorize";
const TOKEN_URL: &str = "https://discord.com/api/oauth2/token";
const USER_URL: &str = "https://discord.com/api/users/@me";

#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DiscordUser {
    id: String,
    username: String,
}

pub async fn login(
    State(state): State<AppState>,
    jar: SignedCookieJar,
) -> Result<(SignedCookieJar, Redirect), AppError> {
    let client = build_oauth_client(&state)?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, csrf_token) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("identify".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    let flow = OauthFlow::new(
        csrf_token.secret().clone(),
        pkce_verifier.secret().clone(),
    );
    let secure = is_secure(&state);
    let jar = jar.add(flow.to_cookie(secure));

    Ok((jar, Redirect::to(auth_url.as_str())))
}

pub async fn callback(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Query(params): Query<CallbackParams>,
) -> Result<(SignedCookieJar, Redirect), AppError> {
    if let Some(err) = params.error {
        return Err(AppError::OauthDenied(err));
    }
    let code = params.code.ok_or(AppError::BadRequest("missing code".into()))?;
    let received_state = params.state.ok_or(AppError::BadRequest("missing state".into()))?;

    // Lift the saved CSRF + PKCE values from the signed cookie.
    let cookie = jar
        .get(STATE_COOKIE)
        .ok_or(AppError::BadRequest("missing oauth state cookie".into()))?;
    let flow: OauthFlow = serde_json::from_str(cookie.value())
        .map_err(|_| AppError::BadRequest("malformed oauth cookie".into()))?;
    if flow.is_expired() {
        return Err(AppError::BadRequest("oauth state expired".into()));
    }
    if !constant_time_eq(flow.csrf_token.as_bytes(), received_state.as_bytes()) {
        return Err(AppError::BadRequest("csrf state mismatch".into()));
    }
    // Drop the now-spent flow cookie regardless of the exchange outcome.
    let jar = jar.remove(clear_cookie(STATE_COOKIE));

    let client = build_oauth_client(&state)?;
    let http = oauth2::reqwest::Client::builder()
        .redirect(oauth2::reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| AppError::Internal(format!("oauth http: {e}")))?;

    let token = client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(PkceCodeVerifier::new(flow.pkce_verifier))
        .request_async(&http)
        .await
        .map_err(|e| AppError::OauthDenied(e.to_string()))?;

    // Use the access token to identify the user.
    let user: DiscordUser = http
        .get(USER_URL)
        .bearer_auth(token.access_token().secret())
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("discord user: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::OauthDenied(e.to_string()))?
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("discord user decode: {e}")))?;

    let user_id: u64 = user
        .id
        .parse()
        .map_err(|_| AppError::Internal("bad discord user id".into()))?;

    // Ask the bot whether this user is an owner. We never trust the client.
    let mut client_rpc = state.rpc.clone();
    let resp = client_rpc
        .check_ownership(Request::new(CheckOwnerRequest { user_id }))
        .await
        .map_err(|e| AppError::Internal(format!("rpc check_ownership: {e}")))?
        .into_inner();
    if !resp.is_owner {
        warn!(user_id, username = %user.username, "non-owner login attempt");
        return Err(AppError::Forbidden);
    }

    info!(user_id, username = %user.username, "admin login");

    let session = Session::new(user_id, user.username, state.config.session_ttl);
    let secure = is_secure(&state);
    let jar = jar.add(session.to_cookie(secure));
    Ok((jar, Redirect::to("/")))
}

pub async fn logout(jar: SignedCookieJar) -> (SignedCookieJar, Redirect) {
    let jar = jar.remove(clear_cookie(COOKIE_NAME));
    (jar, Redirect::to("/"))
}

fn build_oauth_client(state: &AppState) -> Result<BasicClient<
    oauth2::EndpointSet, oauth2::EndpointNotSet, oauth2::EndpointNotSet, oauth2::EndpointNotSet, oauth2::EndpointSet,
>, AppError> {
    let client = BasicClient::new(ClientId::new(state.config.oauth_client_id.clone()))
        .set_client_secret(ClientSecret::new(state.config.oauth_client_secret.clone()))
        .set_auth_uri(
            AuthUrl::new(AUTHORIZE_URL.to_string())
                .map_err(|e| AppError::Internal(format!("auth url: {e}")))?,
        )
        .set_token_uri(
            TokenUrl::new(TOKEN_URL.to_string())
                .map_err(|e| AppError::Internal(format!("token url: {e}")))?,
        )
        .set_redirect_uri(
            RedirectUrl::new(state.config.oauth_redirect_url.clone())
                .map_err(|e| AppError::Internal(format!("redirect url: {e}")))?,
        );
    Ok(client)
}

fn is_secure(state: &AppState) -> bool {
    state
        .config
        .oauth_redirect_url
        .starts_with("https://")
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("forbidden")]
    Forbidden,
    #[error("oauth denied: {0}")]
    OauthDenied(String),
    #[error("internal: {0}")]
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.as_str()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "Sorry, only bot owners can sign in."),
            AppError::OauthDenied(_) => (StatusCode::UNAUTHORIZED, "Discord declined the login."),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong."),
        };
        warn!(error = ?self, "admin error response");
        (status, msg.to_string()).into_response()
    }
}
