use regex::Regex;
use rhai::Map;

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
        // Rhai stores strings as `ImmutableString`, so `read_lock::<String>`
        // returns `None` for string-valued keys here. `try_cast::<String>`
        // handles the conversion — same fix as `loader::meta_string`.
        if let Some(re) = get_string(meta, "regex") {
            return Regex::new(&re)
                .map(Self::Regex)
                .map_err(|e| Error::script(format!("invalid regex: {e}")));
        }
        if get_bool(meta, "has_image") {
            return Ok(Self::HasImage);
        }
        if get_bool(meta, "has_attachment") {
            return Ok(Self::HasAttachment);
        }
        if let Some(s) = get_string(meta, "contains") {
            return Ok(Self::Contains(s));
        }
        if let Some(s) = get_string(meta, "starts_with") {
            return Ok(Self::StartsWith(s));
        }
        if get_bool(meta, "mentions_bot") {
            return Ok(Self::MentionsBot);
        }
        Err(Error::script(
            "trigger meta needs a `match` map with one of: regex, has_image, has_attachment, contains, starts_with, mentions_bot",
        ))
    }
}

fn get_string(meta: &Map, key: &str) -> Option<String> {
    meta.get(key).and_then(|v| v.clone().try_cast::<String>())
}

fn get_bool(meta: &Map, key: &str) -> bool {
    meta.get(key).and_then(|v| v.as_bool().ok()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::TriggerMatcher;
    use rhai::{Engine, Map, Scope};

    fn meta_for(source: &str) -> Map {
        let engine = Engine::new();
        let ast = engine.compile(source).expect("compile");
        let mut scope = Scope::new();
        engine
            .call_fn::<Map>(&mut scope, &ast, "meta", ())
            .expect("meta() call")
            .get("match")
            .expect("`match` key present")
            .clone()
            .try_cast::<Map>()
            .expect("`match` is a map")
    }

    #[test]
    fn parses_regex_matcher() {
        let m = meta_for(r#"fn meta() { #{ "match": #{ regex: "\\bmeow\\b" } } }"#);
        let matcher = TriggerMatcher::from_meta(&m).expect("regex matcher");
        assert!(matches!(matcher, TriggerMatcher::Regex(_)));
    }

    #[test]
    fn parses_has_image_matcher() {
        let m = meta_for(r#"fn meta() { #{ "match": #{ has_image: true } } }"#);
        let matcher = TriggerMatcher::from_meta(&m).expect("has_image matcher");
        assert!(matches!(matcher, TriggerMatcher::HasImage));
    }

    #[test]
    fn parses_contains_matcher() {
        let m = meta_for(r#"fn meta() { #{ "match": #{ contains: "kekw" } } }"#);
        let matcher = TriggerMatcher::from_meta(&m).expect("contains matcher");
        assert!(matches!(matcher, TriggerMatcher::Contains(ref s) if s == "kekw"));
    }
}
