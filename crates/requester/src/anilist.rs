//! AniList lookup over the public GraphQL endpoint.
//!
//! API: <https://graphql.anilist.co>. No key required for read traffic. The
//! single `POST` accepts a `{query, variables}` body; the response has all
//! the fields we need under `data.Page.media[]` (search) or `data.Media`
//! (by id).
//!
//! Two entry points:
//! * [`Requester::anilist_search`] — title search.
//! * [`Requester::anilist_by_id`] — direct id lookup.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::{Error, Requester, Result};

const ENDPOINT: &str = "https://graphql.anilist.co";

/// AniList stores anime and manga under the same `Media` type. We surface
/// this so callers can branch on it (e.g. only embed `episodes` for anime).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AniListType {
    Anime,
    Manga,
}

impl AniListType {
    fn graphql_value(self) -> &'static str {
        match self {
            Self::Anime => "ANIME",
            Self::Manga => "MANGA",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Anime => "anime",
            Self::Manga => "manga",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AniListMedia {
    pub id: u64,
    pub id_mal: Option<u64>,
    #[serde(rename = "type")]
    pub media_type: Option<String>,
    pub format: Option<String>,
    pub status: Option<String>,
    pub episodes: Option<u32>,
    pub chapters: Option<u32>,
    pub volumes: Option<u32>,
    pub duration: Option<u32>,
    pub is_adult: bool,
    pub site_url: Option<String>,
    pub title: AniListTitle,
    pub description: Option<String>,
    pub start_date: Option<AniListDate>,
    pub end_date: Option<AniListDate>,
    pub season: Option<String>,
    pub season_year: Option<u32>,
    pub average_score: Option<u32>,
    pub mean_score: Option<u32>,
    pub popularity: Option<u64>,
    pub favourites: Option<u64>,
    #[serde(default)]
    pub genres: Vec<String>,
    pub cover_image: Option<AniListCover>,
    pub banner_image: Option<String>,
    pub studios: Option<AniListStudioConnection>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AniListTitle {
    pub romaji: Option<String>,
    pub english: Option<String>,
    pub native: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AniListDate {
    pub year: Option<u32>,
    pub month: Option<u32>,
    pub day: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AniListCover {
    pub extra_large: Option<String>,
    pub large: Option<String>,
    /// AniList-provided "dominant colour" hex (`"#9966ff"`) — handy for the
    /// embed accent.
    pub color: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AniListStudioConnection {
    pub nodes: Vec<AniListStudio>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AniListStudio {
    pub name: String,
}

impl AniListMedia {
    /// Best display title: english if present, then romaji, then native.
    /// Falls back to `(no title)` only if AniList returned absolutely nothing.
    pub fn display_title(&self) -> &str {
        self.title
            .english
            .as_deref()
            .or(self.title.romaji.as_deref())
            .or(self.title.native.as_deref())
            .unwrap_or("(no title)")
    }

    pub fn url(&self) -> String {
        self.site_url
            .clone()
            .unwrap_or_else(|| format!("https://anilist.co/anime/{}", self.id))
    }

    pub fn cover_url(&self) -> Option<&str> {
        self.cover_image
            .as_ref()
            .and_then(|c| c.extra_large.as_deref().or(c.large.as_deref()))
    }

    /// AniList's `coverImage.color` parsed as `u32`. Returns `None` if the
    /// field is missing or doesn't look like `#rrggbb`.
    pub fn color_hex(&self) -> Option<u32> {
        let raw = self.cover_image.as_ref().and_then(|c| c.color.as_ref())?;
        let hex = raw.trim().trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()
    }

    /// First "main" studio name, if any.
    pub fn main_studio(&self) -> Option<&str> {
        self.studios
            .as_ref()?
            .nodes
            .first()
            .map(|s| s.name.as_str())
    }

    /// Human-friendly start date — `"2024-04-12"`, `"2024-04"`, `"2024"`, or
    /// `None` when AniList has nothing.
    pub fn start_date_str(&self) -> Option<String> {
        format_date(self.start_date.as_ref())
    }

    pub fn end_date_str(&self) -> Option<String> {
        format_date(self.end_date.as_ref())
    }
}

fn format_date(d: Option<&AniListDate>) -> Option<String> {
    let d = d?;
    match (d.year, d.month, d.day) {
        (Some(y), Some(m), Some(day)) => Some(format!("{y:04}-{m:02}-{day:02}")),
        (Some(y), Some(m), None) => Some(format!("{y:04}-{m:02}")),
        (Some(y), None, None) => Some(y.to_string()),
        _ => None,
    }
}

const FIELDS: &str = r#"
  id
  idMal
  type
  format
  status
  episodes
  chapters
  volumes
  duration
  isAdult
  siteUrl
  title { romaji english native }
  description(asHtml: false)
  startDate { year month day }
  endDate { year month day }
  season
  seasonYear
  averageScore
  meanScore
  popularity
  favourites
  genres
  coverImage { extraLarge large color }
  bannerImage
  studios(isMain: true) { nodes { name } }
"#;

/// Lightweight result shape returned by [`Requester::anilist_search_brief`].
/// Just enough to render a "result N of M" list and identify the entry for
/// a follow-up full fetch, so the multi-result paginator can lazy-load each
/// embed only when the user flips to it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AniListBrief {
    pub id: u64,
    pub title: AniListTitle,
    pub format: Option<String>,
    pub season_year: Option<u32>,
    pub is_adult: bool,
}

impl AniListBrief {
    pub fn display_title(&self) -> &str {
        self.title
            .english
            .as_deref()
            .or(self.title.romaji.as_deref())
            .or(self.title.native.as_deref())
            .unwrap_or("(no title)")
    }

    pub fn url(&self, media_type: AniListType) -> String {
        format!("https://anilist.co/{}/{}", media_type.label(), self.id)
    }
}

#[derive(Deserialize)]
struct PageResponse {
    data: Option<PageData>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

#[derive(Deserialize)]
struct PageData {
    #[serde(rename = "Page")]
    page: Option<Page>,
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    media: Vec<AniListMedia>,
}

#[derive(Deserialize)]
struct BriefPageResponse {
    data: Option<BriefPageData>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

#[derive(Deserialize)]
struct BriefPageData {
    #[serde(rename = "Page")]
    page: Option<BriefPage>,
}

#[derive(Deserialize)]
struct BriefPage {
    #[serde(default)]
    media: Vec<AniListBrief>,
}

#[derive(Deserialize)]
struct MediaResponse {
    data: Option<MediaData>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

#[derive(Deserialize)]
struct MediaData {
    #[serde(rename = "Media")]
    media: Option<AniListMedia>,
}

#[derive(Debug, Deserialize)]
struct GqlError {
    message: String,
}

impl Requester {
    /// Lightweight title search — returns just enough to identify each
    /// result and label it in a paginator. Callers that want the rich body
    /// for a specific entry fetch it via [`Requester::anilist_by_id`].
    ///
    /// AniList allows up to 50 per page; we cap at 25 to keep paginators
    /// responsive.
    pub async fn anilist_search_brief(
        &self,
        query: &str,
        media_type: AniListType,
        limit: u32,
    ) -> Result<Vec<AniListBrief>> {
        let q = query.trim();
        if q.is_empty() {
            return Err(Error::Invalid("empty query".into()));
        }
        let limit = limit.clamp(1, 25);
        let gql = r#"query ($search: String, $type: MediaType, $perPage: Int) {
            Page(page: 1, perPage: $perPage) {
                media(search: $search, type: $type, sort: SEARCH_MATCH) {
                    id
                    isAdult
                    format
                    seasonYear
                    title { romaji english native }
                }
            }
        }"#;
        let body = json!({
            "query": gql,
            "variables": {
                "search": q,
                "type": media_type.graphql_value(),
                "perPage": limit,
            },
        });
        let parsed: BriefPageResponse = self.post_graphql(&body).await?;
        check_errors(&parsed.errors)?;
        Ok(parsed.data.and_then(|d| d.page).map(|p| p.media).unwrap_or_default())
    }

    /// Full-fidelity title search. Prefer
    /// [`Requester::anilist_search_brief`] + per-result
    /// [`Requester::anilist_by_id`] for the paginated command path — this
    /// is here for code that wants every match's body up front.
    pub async fn anilist_search(
        &self,
        query: &str,
        media_type: AniListType,
        limit: u32,
    ) -> Result<Vec<AniListMedia>> {
        let q = query.trim();
        if q.is_empty() {
            return Err(Error::Invalid("empty query".into()));
        }
        let limit = limit.clamp(1, 25);
        let gql = format!(
            r#"query ($search: String, $type: MediaType, $perPage: Int) {{
                Page(page: 1, perPage: $perPage) {{
                    media(search: $search, type: $type, sort: SEARCH_MATCH) {{
                        {FIELDS}
                    }}
                }}
            }}"#
        );
        let body = json!({
            "query": gql,
            "variables": {
                "search": q,
                "type": media_type.graphql_value(),
                "perPage": limit,
            },
        });
        let parsed: PageResponse = self.post_graphql(&body).await?;
        check_errors(&parsed.errors)?;
        Ok(parsed.data.and_then(|d| d.page).map(|p| p.media).unwrap_or_default())
    }

    /// Direct id lookup. Returns `None` when AniList has no media with that
    /// id — that's distinct from an API error.
    pub async fn anilist_by_id(
        &self,
        id: u64,
        media_type: AniListType,
    ) -> Result<Option<AniListMedia>> {
        let gql = format!(
            r#"query ($id: Int, $type: MediaType) {{
                Media(id: $id, type: $type) {{
                    {FIELDS}
                }}
            }}"#
        );
        let body = json!({
            "query": gql,
            "variables": {
                "id": id,
                "type": media_type.graphql_value(),
            },
        });
        // AniList answers a missing id with HTTP 404 (verified) carrying a
        // `{"errors":[{"message":"Not Found."}],"data":{"Media":null}}` body —
        // `post_graphql` turns that non-2xx into `Error::Api { status: 404 }`,
        // so map it to `Ok(None)` instead of surfacing "Lookup failed".
        let parsed: MediaResponse = match self.post_graphql(&body).await {
            Ok(p) => p,
            Err(Error::Api { status: 404, .. }) => return Ok(None),
            Err(e) => return Err(e),
        };
        // Belt-and-suspenders: also handle a 200-with-errors "Not Found" shape
        // in case AniList ever stops using the 404 status.
        if !parsed.errors.is_empty()
            && parsed.errors.iter().any(|e| e.message.contains("Not Found"))
        {
            return Ok(None);
        }
        check_errors(&parsed.errors)?;
        Ok(parsed.data.and_then(|d| d.media))
    }

    async fn post_graphql<T: serde::de::DeserializeOwned>(&self, body: &Value) -> Result<T> {
        let resp = self
            .http
            .post(ENDPOINT)
            .header("accept", "application/json")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(Error::Api { status: status.as_u16(), message });
        }
        resp.json::<T>().await.map_err(|e| Error::Decode(e.to_string()))
    }
}

fn check_errors(errors: &[GqlError]) -> Result<()> {
    if errors.is_empty() {
        Ok(())
    } else {
        let message = errors.iter().map(|e| e.message.as_str()).collect::<Vec<_>>().join("; ");
        Err(Error::Api { status: 200, message })
    }
}

#[cfg(test)]
mod tests {
    use super::{format_date, AniListDate};

    #[test]
    fn formats_full_date() {
        let d = AniListDate { year: Some(2024), month: Some(4), day: Some(12) };
        assert_eq!(format_date(Some(&d)).as_deref(), Some("2024-04-12"));
    }

    #[test]
    fn formats_year_month() {
        let d = AniListDate { year: Some(2024), month: Some(4), day: None };
        assert_eq!(format_date(Some(&d)).as_deref(), Some("2024-04"));
    }

    #[test]
    fn formats_year_only() {
        let d = AniListDate { year: Some(2024), month: None, day: None };
        assert_eq!(format_date(Some(&d)).as_deref(), Some("2024"));
    }

    #[test]
    fn nothing_when_no_year() {
        let d = AniListDate { year: None, month: Some(4), day: Some(12) };
        assert_eq!(format_date(Some(&d)), None);
        assert_eq!(format_date(None), None);
    }
}
