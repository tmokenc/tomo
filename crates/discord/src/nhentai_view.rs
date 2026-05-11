//! Shared embed renderer for nhentai gallery info. Used by both the `/nhentai`
//! command and the auto-trigger that watches NSFW channels for bare numeric
//! messages.

use twilight_model::user::User;

use tomo_embed::Embed2;
use tomo_requester::NhentaiGallery;

use crate::util::truncate;

/// Site favicon used as a footer icon so the embed has a recognisable mark.
const NH_ICON: &str = "https://nhentai.net/favicon.ico";

/// Render an [`NhentaiGallery`] as an [`Embed2`]. `invoker` (the user that
/// triggered the lookup) becomes the embed's author, if present.
pub fn gallery_embed(g: &NhentaiGallery, invoker: Option<&User>) -> Embed2 {
    let mut e = Embed2::lovely()
        .title(truncate(g.best_title(), 256))
        .url(g.page_url())
        .footer_with("nhentai.net", NH_ICON);

    if let Some(user) = invoker {
        e = e.author_user(user);
    }
    if let Some(cover) = g.cover_url.as_ref() {
        e = e.image(cover.clone());
    }

    // Quick-glance stats row.
    e = e.field_inline("ID", format!("`{}`", g.id));
    if let Some(n) = g.page_count {
        e = e.field_inline("Pages", n.to_string());
    }
    if let Some(n) = g.favorites {
        e = e.field_inline("Favs", n.to_string());
    }

    if let Some(jpn) = g.title_japanese.as_ref() {
        e = e.field_block("Japanese", truncate(jpn, 1024));
    }

    let artists: Vec<_> = g.tags_of("artist").collect();
    if !artists.is_empty() {
        e = e.field_block("Artist", truncate(&artists.join(", "), 1024));
    }
    let parodies: Vec<_> = g.tags_of("parody").collect();
    if !parodies.is_empty() {
        e = e.field_block("Parody", truncate(&parodies.join(", "), 1024));
    }
    let characters: Vec<_> = g.tags_of("character").collect();
    if !characters.is_empty() {
        e = e.field_block("Characters", truncate(&characters.join(", "), 1024));
    }
    let langs: Vec<_> = g.tags_of("language").collect();
    if !langs.is_empty() {
        e = e.field_inline("Language", langs.join(", "));
    }
    let categories: Vec<_> = g.tags_of("category").collect();
    if !categories.is_empty() {
        e = e.field_inline("Category", categories.join(", "));
    }
    let groups: Vec<_> = g.tags_of("group").collect();
    if !groups.is_empty() {
        e = e.field_inline("Group", truncate(&groups.join(", "), 1024));
    }
    let tags: Vec<_> = g.tags_of("tag").take(30).collect();
    if !tags.is_empty() {
        e = e.field_block("Tags", truncate(&tags.join(", "), 1024));
    }
    if let Some(when) = g.upload_unix {
        e = e.timestamp_unix(when);
    }
    e
}
