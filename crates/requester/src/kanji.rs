//! Mazii kanji search.
//!
//! Mazii's public dictionary backs the `kanji` command. The endpoint is a
//! plain POST that returns JSON. Each hit covers one kanji with on/kun
//! readings, English/Vietnamese meaning, JLPT level, stroke count, optional
//! detail, and example words for on/kun readings.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{Error, Requester, Result};

const ENDPOINT: &str = "https://mazii.net/api/search";

#[derive(Debug, Clone, Deserialize)]
pub struct KanjiSearch {
    #[serde(default)]
    pub status: u16,
    #[serde(default)]
    pub results: Vec<KanjiEntry>,
}

/// One kanji hit. Field types mirror what Mazii's `javi` dictionary actually
/// returns (verified live): `on` is `null` for kokuji, `level` is a JSON
/// array of JLPT tags like `["N5"]` (or `null`), and `stoke_count` is `null`
/// on this endpoint. Everything optional carries `#[serde(default)]` so a
/// missing *or* explicitly-null field decodes to `None` instead of aborting
/// the whole `Vec<KanjiEntry>` decode.
#[derive(Debug, Clone, Deserialize)]
pub struct KanjiEntry {
    pub kanji: char,
    #[serde(default)]
    pub mean: String,
    #[serde(default)]
    pub on: Option<String>,
    #[serde(default)]
    pub kun: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub comp: Option<String>,
    /// JLPT level tags, e.g. `["N5"]`. Each entry already includes the `N`.
    #[serde(default)]
    pub level: Option<Vec<String>>,
    #[serde(default)]
    pub stoke_count: Option<u32>,
    #[serde(default)]
    pub example_on: Option<HashMap<String, Vec<KanjiExample>>>,
    #[serde(default)]
    pub example_kun: Option<HashMap<String, Vec<KanjiExample>>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KanjiExample {
    #[serde(alias = "w")]
    pub word: String,
    #[serde(alias = "p")]
    pub phonetic: String,
    #[serde(alias = "m")]
    pub meaning: String,
}

#[derive(Serialize)]
struct KanjiBody<'a> {
    dict: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    query: &'a str,
    page: u16,
}

impl KanjiEntry {
    /// `on` reading split by space then re-joined with `、` for prettier output.
    pub fn pretty_on(&self) -> String {
        self.on
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("、")
    }

    /// First JLPT level tag (e.g. `"N5"`), if Mazii reported one.
    pub fn jlpt_level(&self) -> Option<&str> {
        self.level.as_ref()?.first().map(String::as_str)
    }

    pub fn pretty_kun(&self) -> Option<String> {
        self.kun
            .as_ref()
            .map(|k| k.split_whitespace().collect::<Vec<_>>().join("、"))
    }

    /// `detail` uses `##` as a list separator; render it as a markdown list.
    pub fn pretty_detail(&self) -> Option<String> {
        self.detail.as_ref().map(|d| {
            d.split("##")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
    }
}

impl Requester {
    pub async fn kanji_search(&self, query: &str) -> Result<KanjiSearch> {
        let q = query.trim();
        if q.is_empty() {
            return Err(Error::Invalid("kanji query is empty".into()));
        }
        let body = KanjiBody { dict: "javi", kind: "kanji", query: q, page: 1 };
        let mut search: KanjiSearch = self
            .http
            .post(ENDPOINT)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        // Preserve the input order so the user sees kanji in the order they typed.
        search.results.sort_by_key(|entry| {
            q.chars().position(|c| c == entry.kanji).unwrap_or(usize::MAX)
        });
        Ok(search)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real shape returned by `https://mazii.net/api/search` (dict=javi):
    /// `level` is a string array, `on`/`level`/`stoke_count` can be null.
    /// This used to fail to decode entirely, killing the `kanji` command.
    #[test]
    fn decodes_real_mazii_shape() {
        let raw = r#"{
            "status": 200,
            "results": [
                {"kanji":"水","mean":"THỦY","on":"スイ","kun":"みず みず-","level":["N5"],"stoke_count":null},
                {"kanji":"込","mean":"","on":null,"kun":"こ.む","level":["N3"],"stoke_count":null},
                {"kanji":"薔","mean":"SẮC","on":"バ ショウ","kun":"みずたで","level":null,"stoke_count":null}
            ]
        }"#;
        let parsed: KanjiSearch = serde_json::from_str(raw).expect("must decode");
        assert_eq!(parsed.results.len(), 3);
        assert_eq!(parsed.results[0].jlpt_level(), Some("N5"));
        assert_eq!(parsed.results[0].pretty_on(), "スイ");
        // kokuji with a null `on` — must not panic, just yield empty.
        assert_eq!(parsed.results[1].pretty_on(), "");
        assert_eq!(parsed.results[1].jlpt_level(), Some("N3"));
        // null `level` → no tag.
        assert_eq!(parsed.results[2].jlpt_level(), None);
    }
}
