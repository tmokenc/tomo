//! Shared embed renderer for E-Hentai gallery info. Used by both the
//! `/ehentai` command and the auto-trigger that watches NSFW channels for
//! `g/<gid>/<token>/` link patterns.

use twilight_model::user::User;

use tomo_embed::Embed2;
use tomo_requester::EhentaiGallery;

use crate::util::truncate;

/// Site favicon used as a footer icon so the embed has a recognisable mark.
const EH_ICON: &str = "https://e-hentai.org/favicon.ico";

/// Render an [`EhentaiGallery`] as an [`Embed2`]. `invoker` (the user that
/// triggered the lookup) becomes the embed's author, if present.
pub fn gallery_embed(g: &EhentaiGallery, invoker: Option<&User>) -> Embed2 {
    let title = g
        .title
        .as_deref()
        .or(g.title_jpn.as_deref())
        .unwrap_or("(no title)");
    let mut e = Embed2::lovely()
        .title(truncate(title, 256))
        .url(g.url())
        .thumbnail(g.thumb.clone())
        .field_inline("Category", g.category.clone())
        .field_inline("Pages", g.filecount.clone())
        .field_inline("Rating", g.rating.clone())
        .field_inline("Uploader", g.uploader.clone())
        .footer_with("e-hentai.org", EH_ICON);

    if let Some(user) = invoker {
        e = e.author_user(user);
    }
    if let Some(jpn) = &g.title_jpn {
        if jpn != title {
            e = e.field_block("Japanese title", truncate(jpn, 1024));
        }
    }
    if !g.tags.is_empty() {
        let total = g.tags.len();
        let tags = g.tags.iter().take(30).cloned().collect::<Vec<_>>().join(", ");
        let label = if total > 30 {
            format!("Tags ({total} total, top 30)")
        } else {
            format!("Tags ({total})")
        };
        e = e.field_block(label, truncate(&tags, 1024));
    }
    if let Ok(ts) = g.posted.parse::<i64>() {
        e = e.timestamp_unix(ts);
    }
    e
}
