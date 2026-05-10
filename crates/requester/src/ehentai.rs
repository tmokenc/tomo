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

/// Extract `(gid, token)` pairs from a free-form string — picks up
/// `https://e-hentai.org/g/<gid>/<token>/` (and `exhentai.org` variants).
pub fn parse_ids(input: &str) -> Vec<EhentaiId> {
    let mut out = Vec::new();
    for marker in ["e-hentai.org/g/", "exhentai.org/g/"] {
        let mut search = input;
        while let Some(idx) = search.find(marker) {
            let after = &search[idx + marker.len()..];
            let mut parts = after.splitn(3, '/');
            let gid_str = parts.next().unwrap_or("");
            let token = parts.next().unwrap_or("");
            if let Ok(gid) = gid_str.parse::<u64>() {
                if !token.is_empty() && token.chars().all(|c| c.is_ascii_hexdigit()) {
                    out.push((gid, token.to_string()));
                }
            }
            search = &after[gid_str.len()..];
        }
    }
    out
}
