//! Tomo's HTTP requester.
//!
//! Replaces the old `tomoka-rs/requester` crate. The structure here is flat —
//! one module per upstream API, all share a single [`Requester`] that holds
//! the underlying [`reqwest::Client`]. Methods are inherent on `Requester`
//! (no trait gymnastics) so call sites read like plain function calls:
//!
//! ```ignore
//! let r = Requester::new()?;
//! let defs = r.urban_search("waifu").await?;
//! let kanji = r.kanji_search("智花").await?;
//! let posts = r.yandere(&["catgirl"], 5).await?;
//! ```
//!
//! Everything is async, returns typed structs, and bubbles errors as
//! [`Error`] / [`Result`].

pub mod anilist;
pub mod booru;
pub mod ehentai;
pub mod kanji;
pub mod nhentai;
pub mod urban;
pub mod vndb;

pub use anilist::{AniListBrief, AniListMedia, AniListType};
pub use booru::{BooruPost, BooruRating, BooruSource};
pub use ehentai::{EhentaiGallery, EhentaiId};
pub use kanji::{KanjiEntry, KanjiExample, KanjiSearch};
pub use nhentai::{NhentaiGallery, NhentaiTag};
pub use urban::UrbanDefinition;
pub use vndb::{VndbBrief, VndbImage, VndbProducer, VndbTag, VndbVn};

use std::time::Duration;

use reqwest::Client;
use thiserror::Error;

const USER_AGENT: &str = concat!("Tomo/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Error)]
pub enum Error {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("decode: {0}")]
    Decode(String),
    #[error("api error ({status}): {message}")]
    Api { status: u16, message: String },
    #[error("invalid argument: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Cheap-to-clone handle around a `reqwest::Client`. Every API module
/// reaches the network through it.
#[derive(Clone)]
pub struct Requester {
    http: Client,
}

impl Requester {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(USER_AGENT)
            .build()?;
        Ok(Self { http })
    }

    pub fn with_client(http: Client) -> Self {
        Self { http }
    }

    pub fn http(&self) -> &Client {
        &self.http
    }
}

impl Default for Requester {
    fn default() -> Self {
        Self::new().expect("default reqwest client should always build")
    }
}
