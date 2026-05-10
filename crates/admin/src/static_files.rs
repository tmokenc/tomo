//! Serve the compiled Yew SPA. Unknown routes fall back to `index.html` so
//! the SPA router can pick them up client-side.

use std::path::PathBuf;

use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_status::SetStatus;

/// `ServeDir` with `ServeFile` fallback type. The fallback service is wrapped
/// in [`SetStatus`] internally by `not_found_service`, so callers have to
/// surface that in the public type.
pub type SpaService = ServeDir<SetStatus<ServeFile>>;

pub fn router(dist: PathBuf) -> SpaService {
    let index = dist.join("index.html");
    ServeDir::new(dist).not_found_service(ServeFile::new(index))
}
