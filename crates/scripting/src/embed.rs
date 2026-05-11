//! `ScriptEmbed` — Rhai-facing wrapper around [`tomo_embed::Embed2`].
//!
//! Scripts build an embed step by step:
//!
//! ```rhai
//! let e = embed_success();
//! e.title("Done");
//! e.description("It worked.");
//! e.field("foo", "bar", true);
//! e.footer("Tomo bot");
//! e.timestamp_now();
//! ctx.reply_embed(e);
//! ```

use rhai::{CustomType, TypeBuilder};

use tomo_embed::Embed2;

/// Wrapper holding an [`Embed2`] so it can be registered as a custom Rhai
/// type. Methods mutate in place — Rhai scripts call them as statements,
/// not as chained expressions.
#[derive(Debug, Clone, Default)]
pub struct ScriptEmbed {
    inner: Embed2,
}

impl ScriptEmbed {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_embed(inner: Embed2) -> Self {
        Self { inner }
    }

    pub fn info()    -> Self { Self::from_embed(Embed2::info()) }
    pub fn success() -> Self { Self::from_embed(Embed2::success()) }
    pub fn error()   -> Self { Self::from_embed(Embed2::error()) }
    pub fn warning() -> Self { Self::from_embed(Embed2::warning()) }
    pub fn lovely()  -> Self { Self::from_embed(Embed2::lovely()) }

    pub fn into_inner(self) -> Embed2 {
        self.inner
    }

    fn alter(&mut self, f: impl FnOnce(Embed2) -> Embed2) {
        self.inner = f(std::mem::take(&mut self.inner));
    }
}

impl CustomType for ScriptEmbed {
    fn build(mut b: TypeBuilder<Self>) {
        b.with_name("Embed")
            // ---- text ----
            .with_fn("title", |this: &mut ScriptEmbed, v: &str| {
                this.alter(|e| e.title(v.to_string()));
            })
            .with_fn("description", |this: &mut ScriptEmbed, v: &str| {
                this.alter(|e| e.description(v.to_string()));
            })
            .with_fn("url", |this: &mut ScriptEmbed, v: &str| {
                this.alter(|e| e.url(v.to_string()));
            })
            .with_fn("color", |this: &mut ScriptEmbed, c: i64| {
                this.alter(|e| e.color(c as u32));
            })
            // ---- timestamp ----
            .with_fn("timestamp_now", |this: &mut ScriptEmbed| {
                this.alter(Embed2::timestamp_now);
            })
            .with_fn("timestamp_unix", |this: &mut ScriptEmbed, secs: i64| {
                this.alter(|e| e.timestamp_unix(secs));
            })
            // ---- author ----
            .with_fn("author", |this: &mut ScriptEmbed, name: &str| {
                this.alter(|e| e.author(name.to_string()));
            })
            .with_fn(
                "author_with",
                |this: &mut ScriptEmbed, name: &str, icon: &str, url: &str| {
                    let icon = (!icon.is_empty()).then(|| icon.to_string());
                    let url = (!url.is_empty()).then(|| url.to_string());
                    this.alter(|e| e.author_with(name.to_string(), icon, url));
                },
            )
            // ---- footer ----
            .with_fn("footer", |this: &mut ScriptEmbed, v: &str| {
                this.alter(|e| e.footer(v.to_string()));
            })
            .with_fn("footer_with", |this: &mut ScriptEmbed, text: &str, icon: &str| {
                if icon.is_empty() {
                    this.alter(|e| e.footer(text.to_string()));
                } else {
                    this.alter(|e| e.footer_with(text.to_string(), icon.to_string()));
                }
            })
            // ---- fields ----
            .with_fn(
                "field",
                |this: &mut ScriptEmbed, name: &str, value: &str, inline: bool| {
                    this.alter(|e| e.field(name.to_string(), value.to_string(), inline));
                },
            )
            .with_fn("field_inline", |this: &mut ScriptEmbed, name: &str, value: &str| {
                this.alter(|e| e.field_inline(name.to_string(), value.to_string()));
            })
            .with_fn("field_block", |this: &mut ScriptEmbed, name: &str, value: &str| {
                this.alter(|e| e.field_block(name.to_string(), value.to_string()));
            })
            // ---- image / thumbnail ----
            .with_fn("image", |this: &mut ScriptEmbed, url: &str| {
                this.alter(|e| e.image(url.to_string()));
            })
            .with_fn("image_attachment", |this: &mut ScriptEmbed, filename: &str| {
                let f = filename.to_string();
                this.alter(|e| e.image_attachment(f));
            })
            .with_fn("thumbnail", |this: &mut ScriptEmbed, url: &str| {
                this.alter(|e| e.thumbnail(url.to_string()));
            })
            .with_fn("thumbnail_attachment", |this: &mut ScriptEmbed, filename: &str| {
                let f = filename.to_string();
                this.alter(|e| e.thumbnail_attachment(f));
            });
    }
}
