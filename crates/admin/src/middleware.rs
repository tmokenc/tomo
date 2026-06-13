//! Owner-only auth middleware for `/api/*` routes.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::SignedCookieJar;
use tracing::warn;

use crate::session::{Session, COOKIE_NAME};
use crate::state::AppState;

/// Reject the request unless a valid, non-expired session cookie is present
/// **and** the user is still on the bot's live owner list. Attaches the
/// session to request extensions for handlers to read via `Extension<Session>`.
///
/// Ownership is re-checked every request (via gRPC), not just at login — a
/// signed cookie alone would otherwise keep granting access for the whole
/// session TTL (up to a week) even after the user is de-authorised.
pub async fn require_owner(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(cookie) = jar.get(COOKIE_NAME) else {
        return deny(StatusCode::UNAUTHORIZED, "not signed in");
    };
    let session: Session = match serde_json::from_str(cookie.value()) {
        Ok(s) => s,
        Err(_) => return deny(StatusCode::UNAUTHORIZED, "bad session"),
    };
    if session.is_expired() {
        return deny(StatusCode::UNAUTHORIZED, "session expired");
    }

    // Fail closed: if we can't confirm current ownership (RPC down, or the
    // user was removed), deny. The admin UI can't function without the bot's
    // RPC anyway, so denying on an RPC outage costs nothing extra.
    match current_owners(&state).await {
        Ok(owners) if owners.contains(&session.user_id) => {}
        Ok(_) => return deny(StatusCode::FORBIDDEN, "no longer an owner"),
        Err(status) => {
            warn!(code = ?status.code(), "owner re-validation failed — denying");
            return deny(StatusCode::SERVICE_UNAVAILABLE, "could not verify ownership");
        }
    }

    request.extensions_mut().insert(session);
    next.run(request).await
}

/// Fetch the bot's current owner ids over gRPC.
async fn current_owners(state: &AppState) -> Result<Vec<u64>, tonic::Status> {
    let mut client = state.rpc.clone();
    let resp = client
        .get_status(tonic::Request::new(tomo_rpc::Empty {}))
        .await?;
    Ok(resp.into_inner().owners)
}

fn deny(status: StatusCode, msg: &str) -> Response {
    (
        status,
        [("content-type", "application/json")],
        format!(r#"{{"error":"{msg}"}}"#),
    )
        .into_response()
}
