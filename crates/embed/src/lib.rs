//! Embed-building framework for the Tomo bot.
//!
//! Wraps [`twilight_util::builder::embed::EmbedBuilder`] with a fluent,
//! Tomo-flavoured surface:
//!
//! * Opinionated colour helpers (`Embed2::info()`, `::success()`, ...).
//! * Coverage for *every* field a bot-sendable Discord embed can carry —
//!   title, description, url, color, timestamp, author (name/icon/url),
//!   footer (text/icon), thumbnail, image, fields, and attachment-based
//!   image/thumbnail references.
//! * Automatic truncation to Discord's per-field limits so call sites can't
//!   accidentally produce a payload the API will reject.
//! * Hard cap of 25 fields (Discord's limit) — additional fields are
//!   silently dropped.

pub mod color;
pub mod embedable;
pub mod limits;
pub mod util;

pub use color::Color;
pub use embedable::Embedable;
pub use util::truncate;

use chrono::{DateTime, TimeZone, Utc};
use twilight_model::channel::message::Embed;
use twilight_model::id::Id;
use twilight_model::id::marker::UserMarker;
use twilight_model::user::User;
use twilight_model::util::Timestamp;
use twilight_util::builder::embed::{
    EmbedAuthorBuilder, EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder, ImageSource,
};

use crate::limits::*;

/// Tomo's friendly embed builder.
///
/// Internally wraps twilight's [`EmbedBuilder`]; cloning is cheap. Every
/// setter that takes user-provided text auto-truncates to Discord's limit
/// (with an ellipsis) so the resulting embed will not be rejected by the
/// API on length grounds. The 25-field cap is enforced too.
#[derive(Debug, Clone)]
pub struct Embed2 {
    inner: EmbedBuilder,
    field_count: usize,
}

impl Default for Embed2 {
    fn default() -> Self {
        Self::new()
    }
}

impl Embed2 {
    // ---------- Constructors ----------

    pub fn new() -> Self {
        Self {
            inner: EmbedBuilder::new().color(Color::Information.value()),
            field_count: 0,
        }
    }

    pub fn info()    -> Self { Self::new().color(Color::Information) }
    pub fn success() -> Self { Self::new().color(Color::Success) }
    pub fn error()   -> Self { Self::new().color(Color::Error) }
    pub fn warning() -> Self { Self::new().color(Color::Warning) }
    pub fn lovely()  -> Self { Self::new().color(Color::Lovely) }

    // ---------- Top-level text ----------

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.inner = self.inner.title(truncate(&title.into(), TITLE));
        self
    }

    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.inner = self.inner.description(truncate(&desc.into(), DESCRIPTION));
        self
    }

    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.inner = self.inner.url(url.into());
        self
    }

    pub fn color(mut self, color: impl Into<u32>) -> Self {
        self.inner = self.inner.color(color.into());
        self
    }

    // ---------- Timestamp ----------

    pub fn timestamp(mut self, ts: DateTime<Utc>) -> Self {
        if let Ok(t) = Timestamp::from_secs(ts.timestamp()) {
            self.inner = self.inner.timestamp(t);
        }
        self
    }

    pub fn timestamp_now(self) -> Self {
        self.timestamp(Utc::now())
    }

    /// Set the embed timestamp from a Unix epoch second value. Out-of-range
    /// values are silently ignored.
    pub fn timestamp_unix(self, secs: i64) -> Self {
        match Utc.timestamp_opt(secs, 0).single() {
            Some(t) => self.timestamp(t),
            None => self,
        }
    }

    // ---------- Author ----------

    pub fn author(self, name: impl Into<String>) -> Self {
        self.author_with(name, None::<String>, None::<String>)
    }

    pub fn author_with(
        mut self,
        name: impl Into<String>,
        icon_url: Option<impl Into<String>>,
        url: Option<impl Into<String>>,
    ) -> Self {
        let mut builder = EmbedAuthorBuilder::new(truncate(&name.into(), AUTHOR_NAME));
        if let Some(icon) = icon_url {
            if let Ok(src) = ImageSource::url(icon.into()) {
                builder = builder.icon_url(src);
            }
        }
        if let Some(u) = url {
            builder = builder.url(u.into());
        }
        self.inner = self.inner.author(builder.build());
        self
    }

    pub fn author_user(self, user: &User) -> Self {
        let icon = avatar_url(user.id, user.avatar.as_ref().map(ToString::to_string));
        self.author_with(user.name.clone(), icon, None::<String>)
    }

    // ---------- Footer ----------

    pub fn footer(mut self, text: impl Into<String>) -> Self {
        self.inner = self
            .inner
            .footer(EmbedFooterBuilder::new(truncate(&text.into(), FOOTER_TEXT)).build());
        self
    }

    pub fn footer_with(
        mut self,
        text: impl Into<String>,
        icon_url: impl Into<String>,
    ) -> Self {
        let mut footer = EmbedFooterBuilder::new(truncate(&text.into(), FOOTER_TEXT));
        if let Ok(src) = ImageSource::url(icon_url.into()) {
            footer = footer.icon_url(src);
        }
        self.inner = self.inner.footer(footer.build());
        self
    }

    // ---------- Fields ----------

    /// Add a field. Names truncate to 256 chars, values to 1024. After 25
    /// fields are added, further calls are no-ops.
    pub fn field(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
        inline: bool,
    ) -> Self {
        if self.field_count >= MAX_FIELDS {
            return self;
        }
        let mut field = EmbedFieldBuilder::new(
            truncate(&name.into(), FIELD_NAME),
            truncate(&value.into(), FIELD_VALUE),
        );
        if inline {
            field = field.inline();
        }
        self.inner = self.inner.field(field.build());
        self.field_count += 1;
        self
    }

    pub fn field_inline(self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.field(name, value, true)
    }

    pub fn field_block(self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.field(name, value, false)
    }

    /// Add many fields at once. Each tuple is `(name, value, inline)`.
    pub fn fields<I, N, V>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = (N, V, bool)>,
        N: Into<String>,
        V: Into<String>,
    {
        for (name, value, inline) in fields {
            self = self.field(name, value, inline);
        }
        self
    }

    /// How many fields have been added so far (capped at 25).
    pub fn field_count(&self) -> usize {
        self.field_count
    }

    // ---------- Image / thumbnail ----------

    pub fn image(mut self, url: impl Into<String>) -> Self {
        if let Ok(src) = ImageSource::url(url.into()) {
            self.inner = self.inner.image(src);
        }
        self
    }

    /// Use a local attachment as the embed image. The filename must match
    /// the attachment uploaded in the same message.
    pub fn image_attachment(mut self, filename: impl AsRef<str>) -> Self {
        if let Ok(src) = ImageSource::attachment(filename.as_ref()) {
            self.inner = self.inner.image(src);
        }
        self
    }

    pub fn thumbnail(mut self, url: impl Into<String>) -> Self {
        if let Ok(src) = ImageSource::url(url.into()) {
            self.inner = self.inner.thumbnail(src);
        }
        self
    }

    pub fn thumbnail_attachment(mut self, filename: impl AsRef<str>) -> Self {
        if let Ok(src) = ImageSource::attachment(filename.as_ref()) {
            self.inner = self.inner.thumbnail(src);
        }
        self
    }

    // ---------- Escape hatch / build ----------

    /// Drop down to the underlying twilight builder if you need to set
    /// something the wrapper does not expose.
    pub fn with_inner<F>(mut self, f: F) -> Self
    where
        F: FnOnce(EmbedBuilder) -> EmbedBuilder,
    {
        self.inner = f(self.inner);
        self
    }

    pub fn build(self) -> Embed {
        self.inner.build()
    }
}

impl From<Embed2> for Embed {
    fn from(value: Embed2) -> Self {
        value.build()
    }
}

fn avatar_url(user_id: Id<UserMarker>, hash: Option<String>) -> Option<String> {
    let hash = hash?;
    let ext = if hash.starts_with("a_") { "gif" } else { "png" };
    Some(format!("https://cdn.discordapp.com/avatars/{user_id}/{hash}.{ext}?size=256"))
}
