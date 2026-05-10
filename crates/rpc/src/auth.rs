//! Bearer-token interceptors shared by server and client.
//!
//! Why bother on a localhost socket? Defense in depth: if the operator
//! exposes the gRPC port (firewall mishap, container with default-deny that
//! isn't actually default-deny), unauthenticated reads of stats are still
//! gated.

use std::sync::Arc;

use tonic::metadata::AsciiMetadataValue;
use tonic::service::Interceptor;
use tonic::{Request, Status};

const HEADER: &str = "authorization";
const PREFIX: &str = "Bearer ";

/// Server-side interceptor: rejects requests without a matching Bearer token.
/// If `expected` is `None`, the interceptor is a no-op (useful for localhost
/// dev setups).
#[derive(Clone)]
pub struct TokenInterceptor {
    expected: Option<Arc<str>>,
}

impl TokenInterceptor {
    pub fn new(expected: Option<String>) -> Self {
        Self { expected: expected.map(Arc::<str>::from) }
    }

    pub fn intercept<T>(&self, request: Request<T>) -> Result<Request<T>, Status> {
        let Some(expected) = self.expected.as_ref() else {
            return Ok(request);
        };
        let header = request.metadata().get(HEADER).ok_or_else(|| {
            Status::unauthenticated("missing authorization header")
        })?;
        let received = header
            .to_str()
            .map_err(|_| Status::unauthenticated("bad header encoding"))?;
        let token = received
            .strip_prefix(PREFIX)
            .ok_or_else(|| Status::unauthenticated("expected Bearer scheme"))?;
        if constant_time_eq(token.as_bytes(), expected.as_bytes()) {
            Ok(request)
        } else {
            Err(Status::unauthenticated("invalid token"))
        }
    }
}

/// Client-side interceptor — wrap a `Channel` with this when building the
/// client so every outbound call gets a Bearer header (if configured).
#[derive(Clone)]
pub struct BearerInterceptor {
    token: Option<Arc<str>>,
}

impl BearerInterceptor {
    pub fn new(token: Option<String>) -> Self {
        Self { token: token.map(Arc::<str>::from) }
    }
}

impl Interceptor for BearerInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        if let Some(token) = self.token.as_deref() {
            attach_bearer(&mut request, token)?;
        }
        Ok(request)
    }
}

/// Direct helper for callers that want to attach a bearer without going
/// through an interceptor — e.g. a one-off request from an unrelated code
/// path.
pub fn attach_bearer<T>(req: &mut Request<T>, token: &str) -> Result<(), Status> {
    let value = format!("{PREFIX}{token}");
    let parsed: AsciiMetadataValue = value
        .parse()
        .map_err(|_| Status::invalid_argument("token contains non-ascii"))?;
    req.metadata_mut().insert(HEADER, parsed);
    Ok(())
}

/// Constant-time comparison to avoid leaking the token through timing.
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
