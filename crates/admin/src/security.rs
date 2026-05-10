//! Set strict security headers on every response.
//!
//! Applied at the bottom of the middleware stack so it covers static files
//! and API responses alike.

use axum::extract::{Request, State};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;

use crate::state::AppState;

/// CSP that allows the Yew app + a Chart.js CDN. Tighten/loosen here.
const CSP: &str = "default-src 'self'; \
     script-src 'self' 'unsafe-eval' https://cdn.jsdelivr.net; \
     style-src 'self' 'unsafe-inline'; \
     img-src 'self' data: https:; \
     connect-src 'self'; \
     font-src 'self' data:; \
     frame-ancestors 'none'; \
     base-uri 'self'; \
     form-action 'self' https://discord.com";

pub async fn apply_headers(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("content-security-policy", HeaderValue::from_static(CSP));
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert("referrer-policy", HeaderValue::from_static("strict-origin-when-cross-origin"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("permissions-policy", HeaderValue::from_static("geolocation=(), microphone=(), camera=()"));
    if state.config.enable_hsts {
        headers.insert(
            "strict-transport-security",
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }
    response
}
