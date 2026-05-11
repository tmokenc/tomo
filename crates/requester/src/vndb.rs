//! VNDB (Visual Novel Database) lookup.
//!
//! Uses the public Kana API at <https://api.vndb.org/kana>. No API key is
//! required for read traffic. The endpoint accepts a JSON body with a
//! `filters` expression, a comma-separated `fields` selection, plus optional
//! `results`, `page`, `sort`, `reverse`.
//!
//! Two entry points:
//! * [`Requester::vndb_search`] — title search; up to 10 matches by default.
//! * [`Requester::vndb_by_id`] — exact lookup by `"vNNN"` id.

use serde::Deserialize;
use serde_json::json;

use crate::{Error, Requester, Result};

/// `POST` endpoint for VN listings (search + filter).
const VN_URL: &str = "https://api.vndb.org/kana/vn";

/// Fields the bot embeds use. Listed once so search and by-id stay aligned.
/// See <https://api.vndb.org/kana#vn-fields> for the full catalogue.
const FIELDS: &str = "id,title,alttitle,released,languages,length,length_minutes,\
description,image.url,image.sexual,rating,votecount,popularity,\
developers.name,tags.name,tags.category,tags.rating";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VndbVn {
    /// Always `v<num>` (e.g. `"v50283"`).
    pub id: String,
    pub title: String,
    /// Alternate / original-language title, when distinct from `title`.
    pub alttitle: Option<String>,
    /// Languages the VN exists in (ISO codes).
    #[serde(default)]
    pub languages: Vec<String>,
    /// `"YYYY-MM-DD"` or `"TBA"`.
    pub released: Option<String>,
    /// Length bucket — 1 (Very short) … 5 (Very long). `0` / missing = unknown.
    pub length: Option<u8>,
    /// Estimated play time in minutes (community-reported).
    pub length_minutes: Option<u32>,
    pub description: Option<String>,
    pub image: Option<VndbImage>,
    /// Bayesian rating 10–100.
    pub rating: Option<f32>,
    #[serde(default)]
    pub votecount: u32,
    pub popularity: Option<f32>,
    #[serde(default)]
    pub developers: Vec<VndbProducer>,
    #[serde(default)]
    pub tags: Vec<VndbTag>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VndbImage {
    pub url: Option<String>,
    /// 0 = Safe, 1 = Suggestive, 2 = Explicit. (Sexual rating.)
    pub sexual: f32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VndbProducer {
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VndbTag {
    pub name: String,
    /// `"cont"` (content), `"ero"` (sexual content), `"tech"` (technical).
    pub category: String,
    /// 0-3, how strongly the tag applies (community-voted).
    pub rating: f32,
}

impl VndbVn {
    /// Canonical web URL for the VN.
    pub fn url(&self) -> String {
        format!("https://vndb.org/{}", self.id)
    }

    /// Best display title — `alttitle` if it differs, else `title`.
    pub fn display_title(&self) -> &str {
        if self.title.is_empty() {
            self.alttitle.as_deref().unwrap_or("(no title)")
        } else {
            &self.title
        }
    }

    /// Human-friendly length bucket label.
    pub fn length_label(&self) -> Option<&'static str> {
        match self.length? {
            1 => Some("Very short (<2h)"),
            2 => Some("Short (2–10h)"),
            3 => Some("Medium (10–30h)"),
            4 => Some("Long (30–50h)"),
            5 => Some("Very long (50h+)"),
            _ => None,
        }
    }

    /// True if VNDB rates the cover image as sexual / suggestive content.
    pub fn cover_is_nsfw(&self) -> bool {
        self.image.as_ref().map(|i| i.sexual >= 1.0).unwrap_or(false)
    }

    /// Top `n` tags by community rating, optionally filtered by category.
    pub fn top_tags<'a>(
        &'a self,
        category: Option<&'a str>,
        n: usize,
    ) -> Vec<&'a VndbTag> {
        let mut out: Vec<&VndbTag> = self
            .tags
            .iter()
            .filter(|t| category.map(|c| t.category == c).unwrap_or(true))
            .collect();
        out.sort_by(|a, b| b.rating.partial_cmp(&a.rating).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(n);
        out
    }
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    #[serde(default)]
    results: Vec<VndbVn>,
    #[serde(default)]
    #[allow(dead_code)]
    more: bool,
    #[serde(default)]
    #[allow(dead_code)]
    count: Option<u32>,
}

impl Requester {
    /// Title search — returns up to `limit` matches (VNDB caps at 100).
    pub async fn vndb_search(&self, query: &str, limit: u32) -> Result<Vec<VndbVn>> {
        let q = query.trim();
        if q.is_empty() {
            return Err(Error::Invalid("empty query".into()));
        }
        let limit = limit.clamp(1, 25);
        self.vndb_post(json!({
            "filters": ["search", "=", q],
            "fields": FIELDS,
            "results": limit,
            "sort": "searchrank",
        }))
        .await
        .map(|r| r.results)
    }

    /// Look up a single VN by its `vNNN` id. Returns `None` if VNDB has no
    /// such VN (the endpoint returns an empty list, not a 404).
    pub async fn vndb_by_id(&self, id: &str) -> Result<Option<VndbVn>> {
        let id = id.trim();
        if !is_vndb_id(id) {
            return Err(Error::Invalid(format!("not a vndb id: `{id}`")));
        }
        let mut resp = self
            .vndb_post(json!({
                "filters": ["id", "=", id],
                "fields": FIELDS,
                "results": 1,
            }))
            .await?;
        Ok(resp.results.pop())
    }

    async fn vndb_post(&self, body: serde_json::Value) -> Result<ListResponse> {
        let resp = self.http.post(VN_URL).json(&body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(Error::Api { status: status.as_u16(), message });
        }
        resp.json::<ListResponse>().await.map_err(|e| Error::Decode(e.to_string()))
    }
}

/// Cheap shape check: `"v"` followed by 1+ ASCII digits.
fn is_vndb_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 2
        && (bytes[0] == b'v' || bytes[0] == b'V')
        && bytes[1..].iter().all(u8::is_ascii_digit)
}

/// Extract the first `v<digits>` id from arbitrary text — bare id, `vndb.org`
/// URL, or anything else. Returns the *normalised* lowercase id (e.g.
/// `"v50283"`). Used by both the slash command and the auto-trigger.
///
/// To avoid false positives on words ending in `v123` (none come to mind, but
/// still), the `v` must be at the start of the input or follow a non-alphanum
/// character.
pub fn parse_vn_id(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if (bytes[i] == b'v' || bytes[i] == b'V')
            && i + 1 < bytes.len()
            && bytes[i + 1].is_ascii_digit()
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
        {
            let start = i + 1;
            let end = bytes[start..]
                .iter()
                .position(|c| !c.is_ascii_digit())
                .map(|p| start + p)
                .unwrap_or(bytes.len());
            return Some(format!("v{}", &input[start..end]));
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{is_vndb_id, parse_vn_id};

    #[test]
    fn id_shape() {
        assert!(is_vndb_id("v123"));
        assert!(is_vndb_id("V42"));
        assert!(!is_vndb_id("123"));
        assert!(!is_vndb_id("vabc"));
        assert!(!is_vndb_id("v"));
    }

    #[test]
    fn parses_bare_id() {
        assert_eq!(parse_vn_id("v50283").as_deref(), Some("v50283"));
    }

    #[test]
    fn parses_url() {
        assert_eq!(
            parse_vn_id("https://vndb.org/v50283").as_deref(),
            Some("v50283"),
        );
        assert_eq!(parse_vn_id("vndb.org/v123/").as_deref(), Some("v123"));
    }

    #[test]
    fn parses_id_in_sentence() {
        assert_eq!(
            parse_vn_id("yo check this vn: v123 great").as_deref(),
            Some("v123"),
        );
    }

    #[test]
    fn rejects_word_with_v_inside() {
        // "level" should not match — `v` is preceded by `e`.
        assert_eq!(parse_vn_id("level7"), None);
    }

    #[test]
    fn rejects_v_with_no_digits() {
        assert_eq!(parse_vn_id("v hello"), None);
    }

    #[test]
    fn returns_first_id_if_multiple() {
        assert_eq!(parse_vn_id("v1 and v2").as_deref(), Some("v1"));
    }
}
