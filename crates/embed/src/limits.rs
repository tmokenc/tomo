//! Discord embed limits.
//!
//! Values from the Discord API documentation (still current as of 2026-05).
//! These are the per-field character caps; the total-across-the-embed cap
//! of 6 000 chars is enforced by Discord itself — exceeding the per-field
//! limits is the much more common failure mode and is what we trim against.

pub const TITLE:        usize = 256;
pub const DESCRIPTION:  usize = 4096;
pub const FIELD_NAME:   usize = 256;
pub const FIELD_VALUE:  usize = 1024;
pub const FOOTER_TEXT:  usize = 2048;
pub const AUTHOR_NAME:  usize = 256;
pub const MAX_FIELDS:   usize = 25;
pub const TOTAL_CHARS:  usize = 6000;
