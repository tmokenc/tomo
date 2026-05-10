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

#[derive(Debug, Clone, Deserialize)]
pub struct KanjiEntry {
    pub kanji: char,
    pub mean: String,
    pub on: String,
    pub kun: Option<String>,
    pub detail: Option<String>,
    pub comp: Option<String>,
    pub level: Option<char>,
    pub stoke_count: Option<char>,
    pub example_on: Option<HashMap<String, Vec<KanjiExample>>>,
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
        self.on.split_whitespace().collect::<Vec<_>>().join("、")
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
