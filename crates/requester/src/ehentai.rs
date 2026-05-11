//! E-Hentai / ExHentai metadata lookup via the JSON-RPC endpoint at
//! `https://api.e-hentai.org/api.php`.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{Error, Requester, Result};

const API: &str = "https://api.e-hentai.org/api.php";

/// `(gid, token)` pair identifying one gallery.
pub type EhentaiId = (u64, String);

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct EhentaiGallery {
    pub gid: u64,
    pub token: String,
    pub title: Option<String>,
    pub title_jpn: Option<String>,
    pub category: String,
    pub thumb: String,
    pub uploader: String,
    pub posted: String,
    pub filecount: String,
    pub filesize: u64,
    pub expunged: bool,
    pub rating: String,
    pub torrentcount: String,
    pub tags: Vec<String>,
}

impl EhentaiGallery {
    pub fn url(&self) -> String {
        format!("https://e-hentai.org/g/{}/{}/", self.gid, self.token)
    }

    /// True when the gallery is presumed safe-for-work. E-Hentai's tag
    /// model doesn't expose an explicit/safe boolean; we treat only the
    /// `non-h` and `cosplay` categories as safe and consider everything
    /// else NSFW.
    pub fn is_sfw(&self) -> bool {
        let cat = self.category.to_ascii_lowercase();
        cat == "non-h" || cat == "cosplay"
    }
}

#[derive(Deserialize)]
struct GmetadataResponse {
    gmetadata: Vec<EhentaiGallery>,
}

impl Requester {
    /// Hit the `gdata` endpoint with up to 25 `(gid, token)` pairs in one
    /// request — the upstream limit.
    pub async fn ehentai_gmetadata(&self, ids: &[EhentaiId]) -> Result<Vec<EhentaiGallery>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let gidlist: Vec<_> = ids
            .iter()
            .take(25)
            .map(|(gid, token)| json!([gid, token]))
            .collect();

        let body = json!({
            "method": "gdata",
            "gidlist": gidlist,
            "namespace": 1,
        });

        let resp = self
            .http
            .post(API)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        let parsed: GmetadataResponse = resp.json().await.map_err(|e| Error::Decode(e.to_string()))?;
        Ok(parsed.gmetadata)
    }
}

/// Extract `(gid, token)` pairs from a free-form string. Matches both the
/// fully-qualified URL forms (`https://e-hentai.org/g/<gid>/<token>/`,
/// `exhentai.org`) and the bare path form `g/<gid>/<token>/`. Duplicates
/// are removed.
pub fn parse_ids(input: &str) -> Vec<EhentaiId> {
    use std::sync::LazyLock;
    // `\bg/<digits>/<hex>` — word boundary on the left avoids matching
    // unrelated `…rang/123/abc` strings; the token must be at least four
    // hex chars (E-Hentai tokens are 10 by convention but we're lenient).
    static RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\bg/(\d+)/([0-9a-fA-F]{4,40})\b")
            .expect("static ehentai regex")
    });

    let mut out: Vec<EhentaiId> = RE
        .captures_iter(input)
        .filter_map(|caps| {
            let gid = caps.get(1)?.as_str().parse::<u64>().ok()?;
            let token = caps.get(2)?.as_str().to_ascii_lowercase();
            Some((gid, token))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}
