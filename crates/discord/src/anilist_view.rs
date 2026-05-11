//! Shared embed renderer for AniList anime / manga entries. Used by the
//! `/anime` and `/manga` commands; can grow into a trigger later if we
//! want auto-lookups on `anilist.co` URLs.

use twilight_model::user::User;

use tomo_embed::{Color, Embed2};
use tomo_requester::{AniListMedia, AniListType};

use crate::util::truncate;

/// `https://anilist.co/img/icons/favicon-32x32.png` for the footer.
const ANILIST_ICON: &str = "https://anilist.co/img/icons/favicon-32x32.png";

/// Render an [`AniListMedia`] as an [`Embed2`].
///
/// `invoker` becomes the embed's author so it's clear who pulled it up.
/// In SFW channels we drop the cover image when AniList marks the entry as
/// adult — title + url still link out for users who want to follow.
pub fn media_embed(media: &AniListMedia, invoker: Option<&User>, nsfw_channel: bool) -> Embed2 {
    // AniList ships a "dominant colour" for many cover images; use it when
    // present so each embed feels visually distinct, otherwise fall back to
    // the `lovely` accent we use for similar media commands.
    let mut e = match media.color_hex() {
        Some(hex) => Embed2::new().color(Color::Custom(hex)),
        None => Embed2::lovely(),
    };
    e = e
        .title(truncate(media.display_title(), 256))
        .url(media.url())
        .footer_with("anilist.co", ANILIST_ICON);

    if let Some(user) = invoker {
        e = e.author_user(user);
    }
    if let Some(cover) = media.cover_url() {
        if nsfw_channel || !media.is_adult {
            e = e.thumbnail(cover.to_string());
        }
    }
    if let Some(banner) = media.banner_image.as_ref() {
        if nsfw_channel || !media.is_adult {
            e = e.image(banner.clone());
        }
    }
    if let Some(desc) = media.description.as_ref().filter(|s| !s.trim().is_empty()) {
        e = e.description(truncate(&strip_anilist_html(desc), 1500));
    }

    // ── Top row: identifiers ──
    e = e.field_inline("ID", format!("`{}`", media.id));
    if let Some(mal) = media.id_mal {
        e = e.field_inline("MAL", format!("[`{mal}`](https://myanimelist.net/{}/{mal}/)", mal_path(media)));
    }
    if let Some(fmt) = media.format.as_ref() {
        e = e.field_inline("Format", pretty(fmt));
    }
    if let Some(status) = media.status.as_ref() {
        e = e.field_inline("Status", pretty(status));
    }

    // ── Counts ──
    if matches!(media.media_type.as_deref(), Some("ANIME")) {
        if let Some(eps) = media.episodes {
            let label = match media.duration {
                Some(d) if d > 0 => format!("{eps} × {d}m"),
                _ => eps.to_string(),
            };
            e = e.field_inline("Episodes", label);
        }
    } else {
        // MANGA or anything else (novel, oneshot…) — surface chapters/vols.
        if let Some(ch) = media.chapters {
            e = e.field_inline("Chapters", ch.to_string());
        }
        if let Some(v) = media.volumes {
            e = e.field_inline("Volumes", v.to_string());
        }
    }

    // ── Dates ──
    if let Some(start) = media.start_date_str() {
        let value = match media.end_date_str() {
            Some(end) if end != start => format!("{start} → {end}"),
            _ => start,
        };
        e = e.field_inline("Aired", value);
    } else if let (Some(season), Some(year)) = (media.season.as_ref(), media.season_year) {
        e = e.field_inline("Season", format!("{} {year}", pretty(season)));
    }

    // ── Scores ──
    if let Some(score) = media.average_score {
        e = e.field_inline("Score", format!("{score}/100"));
    }
    if let Some(pop) = media.popularity {
        e = e.field_inline("Popularity", pop.to_string());
    }
    if let Some(fav) = media.favourites {
        e = e.field_inline("Favourites", fav.to_string());
    }
    if let Some(studio) = media.main_studio() {
        e = e.field_inline("Studio", studio.to_string());
    }

    // ── Genres ──
    if !media.genres.is_empty() {
        e = e.field_block("Genres", truncate(&media.genres.join(", "), 1024));
    }

    if media.is_adult {
        e = e.footer_with("anilist.co · 🔞 marked adult", ANILIST_ICON);
    }

    e
}

/// AniList's description field is plain text but uses `<br>` for newlines
/// and embeds `<i>`, `<b>`, `<em>`, `<strong>` tags. Strip them — Discord
/// doesn't render HTML and the raw markup looks awful.
fn strip_anilist_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    let mut buf = String::new();
    for ch in s.chars() {
        match ch {
            '<' => {
                in_tag = true;
                buf.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let tag = buf.trim().to_ascii_lowercase();
                let tag = tag.trim_start_matches('/');
                if matches!(tag, "br" | "br/" | "br /") {
                    out.push('\n');
                }
                // Every other tag is dropped — Discord renders text only.
            }
            c if in_tag => buf.push(c),
            c => out.push(c),
        }
    }
    out
}

fn pretty(s: &str) -> String {
    // AniList enums are SCREAMING_SNAKE; humanise them. `TV_SHORT` → `Tv Short`.
    s.split('_')
        .map(|w| {
            let mut out = String::with_capacity(w.len());
            let mut chars = w.chars();
            if let Some(c) = chars.next() {
                out.push(c.to_ascii_uppercase());
                out.push_str(&chars.as_str().to_ascii_lowercase());
            }
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn mal_path(media: &AniListMedia) -> &'static str {
    match media.media_type.as_deref() {
        Some("MANGA") => "manga",
        _ => "anime",
    }
}

/// Build a deliberately-thin embed for "result N of M" in a paginator. The
/// full embed renders lazily once the user lands on the page (see
/// [`crate::command::builtin::anime`]).
pub fn brief_embed(
    brief: &tomo_requester::AniListBrief,
    media_type: AniListType,
    invoker: Option<&User>,
    index: usize,
    total: usize,
) -> Embed2 {
    let mut e = Embed2::info()
        .title(truncate(brief.display_title(), 256))
        .url(brief.url(media_type))
        .description(format!(
            "Result **{}** of **{total}**. Use the buttons to flip — each entry is fetched when you reach it.",
            index + 1,
        ))
        .field_inline("ID", format!("`{}`", brief.id))
        .footer_with("anilist.co · loading…", ANILIST_ICON);
    if let Some(fmt) = brief.format.as_ref() {
        e = e.field_inline("Format", pretty(fmt));
    }
    if let Some(year) = brief.season_year {
        e = e.field_inline("Year", year.to_string());
    }
    if brief.is_adult {
        e = e.field_inline("🔞", "marked adult");
    }
    if let Some(user) = invoker {
        e = e.author_user(user);
    }
    e
}

#[cfg(test)]
mod tests {
    use super::{pretty, strip_anilist_html};

    #[test]
    fn humanises_screaming_snake() {
        assert_eq!(pretty("TV_SHORT"), "Tv Short");
        assert_eq!(pretty("FINISHED"), "Finished");
        assert_eq!(pretty("ONA"), "Ona");
    }

    #[test]
    fn strips_br_to_newline() {
        let s = "Line one<br>Line two<br/>Line three";
        assert_eq!(strip_anilist_html(s), "Line one\nLine two\nLine three");
    }

    #[test]
    fn drops_italic_and_bold() {
        assert_eq!(
            strip_anilist_html("<i>spicy</i> and <b>bold</b>"),
            "spicy and bold",
        );
    }
}
