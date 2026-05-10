//! Urban Dictionary v0 API.
//!
//! No auth required. Two endpoints we care about:
//! - `define?term=<word>` — definitions for a word.
//! - `random`             — a handful of random entries.

use serde::Deserialize;

use crate::{Error, Requester, Result};

const DEFINE_URL: &str = "https://api.urbandictionary.com/v0/define";
const RANDOM_URL: &str = "https://api.urbandictionary.com/v0/random";

#[derive(Debug, Clone, Deserialize)]
pub struct UrbanDefinition {
    pub word: String,
    pub definition: String,
    pub permalink: String,
    pub author: String,
    pub example: String,
    pub thumbs_up: u32,
    pub thumbs_down: u32,
    pub written_on: String,
}

#[derive(Deserialize)]
struct UrbanRoot {
    list: Vec<UrbanDefinition>,
}

impl Requester {
    pub async fn urban_search(&self, term: &str) -> Result<Vec<UrbanDefinition>> {
        if term.trim().is_empty() {
            return Err(Error::Invalid("term is empty".into()));
        }
        let root: UrbanRoot = self
            .http
            .get(DEFINE_URL)
            .query(&[("term", term)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(root.list)
    }

    pub async fn urban_random(&self) -> Result<Vec<UrbanDefinition>> {
        let root: UrbanRoot = self
            .http
            .get(RANDOM_URL)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(root.list)
    }
}
