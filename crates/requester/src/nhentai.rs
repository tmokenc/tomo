//! nhentai gallery lookup — by scraping the public `/g/<id>/` HTML.
//!
//! The legacy `/api/gallery/<id>` JSON endpoint is dead. We pull the same
//! information out of the page itself with [`scraper`]. The bot only needs
//! the cover image, title(s), tag groups, page count, upload date and
//! favourites count — none of which require the full per-page metadata.

use std::sync::LazyLock;

use scraper::{ElementRef, Html, Selector};

use crate::{Error, Requester, Result};

/// nhentai serves a CF challenge to obvious bot user agents — pretend to be
/// a recent Firefox by default.
const BROWSER_UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

#[derive(Debug, Clone, Default)]
pub struct NhentaiGallery {
    pub id: u64,
    pub title_english: Option<String>,
    pub title_japanese: Option<String>,
    pub title_pretty: Option<String>,
    pub cover_url: Option<String>,
    pub page_count: Option<u32>,
    pub favorites: Option<u32>,
    pub upload_unix: Option<i64>,
    pub tags: Vec<NhentaiTag>,
}

#[derive(Debug, Clone)]
pub struct NhentaiTag {
    /// `"parody" | "character" | "tag" | "artist" | "group" | "language" | "category"`
    pub kind: String,
    pub name: String,
}

impl NhentaiGallery {
    pub fn page_url(&self) -> String {
        format!("https://nhentai.net/g/{}/", self.id)
    }

    pub fn tags_of<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.tags
            .iter()
            .filter(move |t| t.kind == kind)
            .map(|t| t.name.as_str())
    }

    pub fn best_title(&self) -> &str {
        self.title_pretty
            .as_deref()
            .or(self.title_english.as_deref())
            .or(self.title_japanese.as_deref())
            .unwrap_or("(no title)")
    }
}

// Selectors are compiled once.
static S_TITLE_PRETTY: LazyLock<Selector> = LazyLock::new(|| Selector::parse("h1.title .pretty").unwrap());
static S_TITLE_BEFORE: LazyLock<Selector> = LazyLock::new(|| Selector::parse("h1.title .before").unwrap());
static S_TITLE_AFTER: LazyLock<Selector> = LazyLock::new(|| Selector::parse("h1.title .after").unwrap());
static S_TITLE_JPN: LazyLock<Selector> = LazyLock::new(|| Selector::parse("h2.title .pretty").unwrap());
static S_COVER_IMG: LazyLock<Selector> = LazyLock::new(|| Selector::parse("#cover img").unwrap());
static S_TAG_CONTAINER: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("#tags .tag-container").unwrap());
static S_TAG_NAME: LazyLock<Selector> = LazyLock::new(|| Selector::parse(".tag .name").unwrap());
static S_TIME: LazyLock<Selector> = LazyLock::new(|| Selector::parse("#tags time").unwrap());

impl Requester {
    pub async fn nhentai(&self, id: u64) -> Result<NhentaiGallery> {
        let url = format!("https://nhentai.net/g/{id}/");
        let resp = self
            .http
            .get(&url)
            .header("user-agent", BROWSER_UA)
            .header("accept", "text/html")
            .send()
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::Api { status: 404, message: format!("gallery {id} not found") });
        }
        let html = resp.error_for_status()?.text().await?;
        Ok(parse_gallery_page(id, &html))
    }
}

fn parse_gallery_page(id: u64, html: &str) -> NhentaiGallery {
    let document = Html::parse_document(html);

    let title_pretty = first_text(&document, &S_TITLE_PRETTY);
    let title_english = {
        let before = first_text(&document, &S_TITLE_BEFORE).unwrap_or_default();
        let mid = title_pretty.clone().unwrap_or_default();
        let after = first_text(&document, &S_TITLE_AFTER).unwrap_or_default();
        let combined = format!("{before}{mid}{after}").trim().to_string();
        (!combined.is_empty()).then_some(combined)
    };
    let title_japanese = first_text(&document, &S_TITLE_JPN);

    let cover_url = document.select(&S_COVER_IMG).next().and_then(|img| {
        img.value()
            .attr("data-src")
            .or_else(|| img.value().attr("src"))
            .map(str::to_string)
    });

    let mut tags = Vec::new();
    let mut page_count: Option<u32> = None;
    let mut favorites: Option<u32> = None;

    for container in document.select(&S_TAG_CONTAINER) {
        let Some(kind) = container_kind(container) else { continue };
        match kind.as_str() {
            "pages" => {
                page_count = container
                    .select(&S_TAG_NAME)
                    .next()
                    .and_then(|n| n.text().collect::<String>().trim().parse::<u32>().ok());
            }
            "favorites" => {
                favorites = container
                    .select(&S_TAG_NAME)
                    .next()
                    .and_then(|n| n.text().collect::<String>().trim().parse::<u32>().ok());
            }
            "uploaded" => {
                // The `<time>` element carries the precise instant.
            }
            kind_for_tags => {
                let singular = singularise_tag_kind(kind_for_tags);
                for name in container.select(&S_TAG_NAME) {
                    let text = name.text().collect::<String>().trim().to_string();
                    if !text.is_empty() {
                        tags.push(NhentaiTag { kind: singular.clone(), name: text });
                    }
                }
            }
        }
    }

    let upload_unix = document
        .select(&S_TIME)
        .next()
        .and_then(|t| t.value().attr("datetime"))
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp());

    NhentaiGallery {
        id,
        title_english,
        title_japanese,
        title_pretty,
        cover_url,
        page_count,
        favorites,
        upload_unix,
        tags,
    }
}

fn first_text(doc: &Html, sel: &Selector) -> Option<String> {
    doc.select(sel).next().map(|el| el.text().collect::<String>().trim().to_string())
}

/// The label of a `.tag-container` is the leading text node, e.g.
/// `"Tags:"`, `"Pages:"`, `"Uploaded:"`. Returns it lowercased + trimmed.
fn container_kind(container: ElementRef<'_>) -> Option<String> {
    for node in container.children() {
        if let Some(text) = node.value().as_text() {
            let trimmed = text.trim().trim_end_matches(':');
            if !trimmed.is_empty() {
                return Some(trimmed.to_ascii_lowercase());
            }
        }
    }
    None
}

fn singularise_tag_kind(kind: &str) -> String {
    match kind {
        "parodies" => "parody".into(),
        "characters" => "character".into(),
        "tags" => "tag".into(),
        "artists" => "artist".into(),
        "groups" => "group".into(),
        "languages" => "language".into(),
        "categories" => "category".into(),
        other => other.trim_end_matches('s').to_string(),
    }
}

/// Extract a gallery id from a free-form string: a bare number, or a URL of
/// the form `nhentai.net/g/<id>` (with or without trailing `/`). Returns
/// `None` if nothing usable is found.
pub fn parse_gallery_id(input: &str) -> Option<u64> {
    if let Ok(n) = input.trim().parse::<u64>() {
        return Some(n);
    }
    let bytes = input.as_bytes();
    let needle = b"/g/";
    for i in 0..bytes.len().saturating_sub(needle.len()) {
        if &bytes[i..i + needle.len()] == needle {
            let rest = &input[i + needle.len()..];
            let id_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(id) = id_str.parse::<u64>() {
                return Some(id);
            }
        }
    }
    None
}
