//! Booru clients.
//!
//! Three sites are wired up — yande.re, konachan, and danbooru. The first
//! two share the *Moebooru* JSON shape; danbooru's is similar but field
//! names diverge. The common surface this module exports is [`BooruPost`].

use serde::Deserialize;
use tracing::warn;

use crate::{Error, Requester, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooruSource {
    Yandere,
    Konachan,
    Danbooru,
}

impl BooruSource {
    /// Parse a user-friendly name (`"yandere"`, `"konachan"`, `"danbooru"`
    /// and a few short aliases). Returns `None` if nothing matches — the
    /// command falls back to the default source in that case.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "yandere" | "yande.re" | "yan" => Self::Yandere,
            "konachan" | "kona" | "kon" => Self::Konachan,
            "danbooru" | "dan" | "danb" => Self::Danbooru,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Yandere => "yande.re",
            Self::Konachan => "konachan.com",
            Self::Danbooru => "danbooru.donmai.us",
        }
    }

    fn list_url(self) -> &'static str {
        match self {
            Self::Yandere => "https://yande.re/post.json",
            Self::Konachan => "https://konachan.com/post.json",
            Self::Danbooru => "https://danbooru.donmai.us/posts.json",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooruRating {
    Safe,
    Questionable,
    Explicit,
}

impl BooruRating {
    pub fn from_char(c: char) -> Self {
        match c.to_ascii_lowercase() {
            'e' => Self::Explicit,
            'q' => Self::Questionable,
            _ => Self::Safe,
        }
    }
}

/// Unified booru post — what every command and embed deals with.
#[derive(Debug, Clone)]
pub struct BooruPost {
    pub source: BooruSource,
    pub id: u64,
    pub tags: String,
    pub author: String,
    pub source_url: Option<String>,
    pub file_url: String,
    pub preview_url: Option<String>,
    pub rating: BooruRating,
    pub width: u32,
    pub height: u32,
    pub score: i32,
}

impl Requester {
    pub async fn booru(
        &self,
        source: BooruSource,
        tags: &[&str],
        limit: u32,
    ) -> Result<Vec<BooruPost>> {
        let limit = limit.clamp(1, 50);
        match source {
            BooruSource::Yandere | BooruSource::Konachan => {
                self.fetch_moebooru(source, tags, limit).await
            }
            BooruSource::Danbooru => self.fetch_danbooru(tags, limit).await,
        }
    }

    pub async fn yandere(&self, tags: &[&str], limit: u32) -> Result<Vec<BooruPost>> {
        self.booru(BooruSource::Yandere, tags, limit).await
    }

    pub async fn konachan(&self, tags: &[&str], limit: u32) -> Result<Vec<BooruPost>> {
        self.booru(BooruSource::Konachan, tags, limit).await
    }

    pub async fn danbooru(&self, tags: &[&str], limit: u32) -> Result<Vec<BooruPost>> {
        self.booru(BooruSource::Danbooru, tags, limit).await
    }

    async fn fetch_moebooru(
        &self,
        source: BooruSource,
        tags: &[&str],
        limit: u32,
    ) -> Result<Vec<BooruPost>> {
        let tag_str = tags.join(" ");
        let url = source.list_url();
        let resp: Vec<MoebooruPost> = self
            .http
            .get(url)
            .query(&[("tags", tag_str.as_str()), ("limit", &limit.to_string())])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(resp.into_iter().map(|p| p.into_post(source)).collect())
    }

    async fn fetch_danbooru(&self, tags: &[&str], limit: u32) -> Result<Vec<BooruPost>> {
        let tag_str = tags.join(" ");
        let url = BooruSource::Danbooru.list_url();
        let resp = self
            .http
            .get(url)
            .query(&[("tags", tag_str.as_str()), ("limit", &limit.to_string())])
            .send()
            .await?
            .error_for_status()?;

        let posts: Vec<DanbooruPost> = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "danbooru decode failed");
                return Err(Error::Decode(e.to_string()));
            }
        };
        Ok(posts.into_iter().filter_map(|p| p.into_post()).collect())
    }
}

// ---------- Moebooru wire types ----------

#[derive(Deserialize)]
struct MoebooruPost {
    id: u64,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    author: String,
    source: Option<String>,
    score: i32,
    file_url: String,
    preview_url: Option<String>,
    #[serde(default = "default_rating")]
    rating: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

fn default_rating() -> String {
    "s".into()
}

impl MoebooruPost {
    fn into_post(self, source: BooruSource) -> BooruPost {
        let rating = self.rating.chars().next().map(BooruRating::from_char).unwrap_or(BooruRating::Safe);
        BooruPost {
            source,
            id: self.id,
            tags: self.tags,
            author: self.author,
            source_url: self.source,
            file_url: self.file_url,
            preview_url: self.preview_url,
            rating,
            width: self.width,
            height: self.height,
            score: self.score,
        }
    }
}

// ---------- Danbooru wire types ----------

#[derive(Deserialize)]
struct DanbooruPost {
    id: u64,
    #[serde(default)]
    tag_string: String,
    #[serde(default)]
    uploader_name: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    score: i32,
    file_url: Option<String>,
    preview_file_url: Option<String>,
    #[serde(default)]
    rating: Option<String>,
    #[serde(default)]
    image_width: u32,
    #[serde(default)]
    image_height: u32,
}

impl DanbooruPost {
    fn into_post(self) -> Option<BooruPost> {
        let file_url = self.file_url?;
        let rating = self
            .rating
            .as_deref()
            .and_then(|r| r.chars().next())
            .map(BooruRating::from_char)
            .unwrap_or(BooruRating::Safe);
        Some(BooruPost {
            source: BooruSource::Danbooru,
            id: self.id,
            tags: self.tag_string,
            author: self.uploader_name.unwrap_or_default(),
            source_url: self.source,
            file_url,
            preview_url: self.preview_file_url,
            rating,
            width: self.image_width,
            height: self.image_height,
            score: self.score,
        })
    }
}
