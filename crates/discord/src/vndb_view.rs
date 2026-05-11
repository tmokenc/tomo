//! Shared embed renderer for VNDB visual novels. Used by `/vndb` (single
//! and paginated views) and the `vN`-id auto-trigger.

use twilight_model::user::User;

use tomo_embed::Embed2;
use tomo_requester::VndbVn;

use crate::util::truncate;

/// `https://s.vndb.org` favicon — branded footer icon on every embed.
const VNDB_ICON: &str = "https://s.vndb.org/s/angel.png";

/// Render a [`VndbVn`] as an [`Embed2`].
///
/// `invoker` (if present) becomes the embed's author so it's clear who
/// triggered the lookup. `nsfw_channel` controls whether the cover image is
/// shown when VNDB rates it as suggestive/explicit — in SFW channels we drop
/// the image instead of risking a TOS bite.
pub fn gallery_embed(vn: &VndbVn, invoker: Option<&User>, nsfw_channel: bool) -> Embed2 {
    let mut e = Embed2::lovely()
        .title(truncate(vn.display_title(), 256))
        .url(vn.url())
        .footer_with("vndb.org", VNDB_ICON);

    if let Some(user) = invoker {
        e = e.author_user(user);
    }

    // Cover: only show if SFW channel, or VNDB rates it as safe.
    if let Some(url) = vn.image.as_ref().and_then(|i| i.url.as_ref()) {
        if nsfw_channel || !vn.cover_is_nsfw() {
            e = e.image(url.clone());
        }
    }

    if let Some(desc) = vn.description.as_ref().filter(|s| !s.is_empty()) {
        // VNDB descriptions can carry [url=...][/url] BBCode — strip the
        // markers so they don't render literally in Discord.
        e = e.description(truncate(&strip_bbcode(desc), 1500));
    }

    // ID is always useful — small inline field at the top of the row.
    e = e.field_inline("ID", format!("`{}`", vn.id));

    if let Some(rel) = vn.released.as_ref().filter(|s| !s.is_empty()) {
        e = e.field_inline("Released", rel.clone());
    }
    if let Some(label) = vn.length_label() {
        let mut value = label.to_string();
        if let Some(mins) = vn.length_minutes.filter(|m| *m > 0) {
            value.push_str(&format!(" · ~{}h", (mins as f32 / 60.0).round() as u32));
        }
        e = e.field_inline("Length", value);
    }
    if let Some(rating) = vn.rating {
        let votes = if vn.votecount > 0 {
            format!(" ({}v)", vn.votecount)
        } else {
            String::new()
        };
        e = e.field_inline("Rating", format!("{rating:.1}/100{votes}"));
    }
    if let Some(pop) = vn.popularity {
        e = e.field_inline("Popularity", format!("{pop:.1}"));
    }

    if vn.alttitle.as_deref().is_some_and(|a| a != vn.title) {
        if let Some(alt) = vn.alttitle.as_ref() {
            e = e.field_block("Original title", truncate(alt, 1024));
        }
    }

    if !vn.developers.is_empty() {
        let names = vn
            .developers
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        e = e.field_block("Developers", truncate(&names, 1024));
    }

    if !vn.languages.is_empty() {
        e = e.field_inline(
            "Languages",
            truncate(&vn.languages.join(", "), 1024),
        );
    }

    let content_tags = vn.top_tags(Some("cont"), 12);
    if !content_tags.is_empty() {
        let body = content_tags
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        e = e.field_block("Tags", truncate(&body, 1024));
    }

    e
}

/// Strip the most common VNDB BBCode markup (`[url=...]...[/url]`,
/// `[spoiler]...[/spoiler]`, `[b]...[/b]`, `[i]...[/i]`) so descriptions
/// render as plain text in Discord. Not a full BBCode parser — just enough to
/// stop tags showing up literally.
fn strip_bbcode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close) = s[i..].find(']') {
                let tag = &s[i + 1..i + close];
                // Keep `[url=...]anchor[/url]` as just the anchor text.
                if let Some(stripped) = tag.strip_prefix("url=") {
                    let anchor_start = i + close + 1;
                    if let Some(end_rel) = s[anchor_start..].find("[/url]") {
                        let anchor = &s[anchor_start..anchor_start + end_rel];
                        out.push_str(anchor);
                        out.push_str(" (");
                        out.push_str(stripped);
                        out.push(')');
                        i = anchor_start + end_rel + "[/url]".len();
                        continue;
                    }
                }
                // Generic tag — drop it entirely.
                if tag.starts_with('/') || matches!(tag, "b" | "i" | "u" | "s" | "spoiler" | "code" | "quote") {
                    i += close + 1;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::strip_bbcode;

    #[test]
    fn strips_bold() {
        assert_eq!(strip_bbcode("hello [b]world[/b]!"), "hello world!");
    }

    #[test]
    fn keeps_url_anchor_with_target_in_parens() {
        assert_eq!(
            strip_bbcode("see [url=https://x.com]this[/url] please"),
            "see this (https://x.com) please",
        );
    }

    #[test]
    fn drops_spoiler() {
        assert_eq!(strip_bbcode("[spoiler]big reveal[/spoiler]"), "big reveal");
    }
}
