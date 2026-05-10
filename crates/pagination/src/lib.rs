//! Button-driven pagination.
//!
//! A [`PageSource`] yields embeds (eagerly from a `Vec`, lazily from a
//! callback, or however else you want). [`Paginator`] sends the first page,
//! attaches first/prev/next/last/close buttons, and listens for component
//! interactions through `twilight-standby`. After `idle_timeout` the buttons
//! are disabled and the listener returns.

pub mod page_source;
pub mod paginator;

pub use page_source::{PageSource, VecPageSource};
pub use paginator::{Paginator, PaginatorOptions};
