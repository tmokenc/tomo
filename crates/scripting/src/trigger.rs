use regex::Regex;
use rhai::{Dynamic, Map};

use tomo_core::error::{Error, Result};

/// What kinds of message events a trigger can match against. New variants are
/// cheap to add — wire them into `evaluate` and surface them in `from_meta`.
#[derive(Debug, Clone)]
pub enum TriggerMatcher {
    /// Regex matches anywhere in the message content.
    Regex(Regex),
    /// Message has at least one image attachment.
    HasImage,
    /// Message has any attachment.
    HasAttachment,
    /// Plain substring match (case-insensitive).
    Contains(String),
    /// Message starts with the given string (case-insensitive).
    StartsWith(String),
    /// Message mentions the bot.
    MentionsBot,
}

#[derive(Debug, Clone, Default)]
pub struct MessageProbe<'a> {
    pub content: &'a str,
    pub has_image: bool,
    pub has_attachment: bool,
    pub mentions_bot: bool,
}

impl TriggerMatcher {
    pub fn matches(&self, probe: &MessageProbe<'_>) -> bool {
        match self {
            Self::Regex(re) => re.is_match(probe.content),
            Self::HasImage => probe.has_image,
            Self::HasAttachment => probe.has_attachment,
            Self::Contains(s) => probe.content.to_ascii_lowercase().contains(&s.to_ascii_lowercase()),
            Self::StartsWith(s) => probe
                .content
                .trim_start()
                .to_ascii_lowercase()
                .starts_with(&s.to_ascii_lowercase()),
            Self::MentionsBot => probe.mentions_bot,
        }
    }

    /// Build a matcher from the `match` field of a script's `meta()` map.
    ///
    /// Accepted shapes:
    /// ```rhai
    /// match: #{ regex: "(?i)hi" }
    /// match: #{ has_image: true }
    /// match: #{ has_attachment: true }
    /// match: #{ contains: "meow" }
    /// match: #{ starts_with: "!"   }
    /// match: #{ mentions_bot: true }
    /// ```
    pub fn from_meta(meta: &Map) -> Result<Self> {
        if let Some(re) = meta.get("regex").and_then(Dynamic::read_lock::<String>) {
            return Regex::new(&re)
                .map(Self::Regex)
                .map_err(|e| Error::script(format!("invalid regex: {e}")));
        }
        if meta.get("has_image").map(Dynamic::as_bool).and_then(Result::ok).unwrap_or(false) {
            return Ok(Self::HasImage);
        }
        if meta.get("has_attachment").map(Dynamic::as_bool).and_then(Result::ok).unwrap_or(false) {
            return Ok(Self::HasAttachment);
        }
        if let Some(s) = meta.get("contains").and_then(Dynamic::read_lock::<String>) {
            return Ok(Self::Contains(s.clone()));
        }
        if let Some(s) = meta.get("starts_with").and_then(Dynamic::read_lock::<String>) {
            return Ok(Self::StartsWith(s.clone()));
        }
        if meta.get("mentions_bot").map(Dynamic::as_bool).and_then(Result::ok).unwrap_or(false) {
            return Ok(Self::MentionsBot);
        }
        Err(Error::script(
            "trigger meta needs a `match` map with one of: regex, has_image, has_attachment, contains, starts_with, mentions_bot",
        ))
    }
}
