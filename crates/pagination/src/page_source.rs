use async_trait::async_trait;
use twilight_model::channel::message::Embed;

use tomo_core::error::Result;

/// Yields embeds one page at a time. Implementations may keep state internally
/// — e.g. a cursor into a paginated API response.
#[async_trait]
pub trait PageSource: Send + Sync {
    /// Total page count if known. `None` means unbounded and disables the
    /// "last" button.
    fn total_pages(&self) -> Option<usize>;

    /// Return the embed for page `index` (zero-based).
    async fn page(&self, index: usize) -> Result<Embed>;
}

/// Simplest backing: a precomputed `Vec<Embed>`.
pub struct VecPageSource {
    pages: Vec<Embed>,
}

impl VecPageSource {
    pub fn new(pages: Vec<Embed>) -> Self {
        Self { pages }
    }
}

#[async_trait]
impl PageSource for VecPageSource {
    fn total_pages(&self) -> Option<usize> {
        Some(self.pages.len())
    }

    async fn page(&self, index: usize) -> Result<Embed> {
        Ok(self
            .pages
            .get(index)
            .cloned()
            .unwrap_or_else(|| Embed {
                kind: "rich".into(),
                title: Some("Out of range".into()),
                ..default_embed()
            }))
    }
}

fn default_embed() -> Embed {
    Embed {
        author: None,
        color: None,
        description: None,
        fields: Vec::new(),
        footer: None,
        image: None,
        kind: "rich".into(),
        provider: None,
        thumbnail: None,
        timestamp: None,
        title: None,
        url: None,
        video: None,
    }
}
