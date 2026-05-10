//! Owner-only auth middleware for `/api/*` routes.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::SignedCookieJar;

use crate::session::{Session, COOKIE_NAME};
use crate::state::AppState;

/// Reject the request unless a valid, non-expired, owner-issued session
/// cookie is present. Attaches the session to request extensions for handlers
/// to read via `Extension<Session>`.
pub async fn require_owner(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    mut request: Request,
    next: Next,
) -> Response {
    let _ = state; // reserved for re-checking ownership via gRPC on long sessions
    let Some(cookie) = jar.get(COOKIE_NAME) else {
        return unauthorised("not signed in");
    };
    let session: Session = match serde_json::from_str(cookie.value()) {
        Ok(s) => s,
        Err(_) => return unauthorised("bad session"),
    };
    if session.is_expired() {
        return unauthorised("session expired");
    }
    request.extensions_mut().insert(session);
    next.run(request).await
}

fn unauthorised(msg: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("content-type", "application/json")],
        format!(r#"{{"error":"{msg}"}}"#),
    )
        .into_response()
}
