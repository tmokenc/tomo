use twilight_model::channel::message::Embed;

use crate::Embed2;

/// Anything that can be rendered as a Discord embed.
///
/// Implementing this lets a value be passed directly to the response helpers
/// in `tomo-discord` — `ctx.reply_embed(my_value)` instead of writing the
/// embed by hand at every call site.
pub trait Embedable {
    fn build_embed(&self) -> Embed;
}

impl Embedable for Embed2 {
    fn build_embed(&self) -> Embed {
        self.clone().build()
    }
}

impl Embedable for Embed {
    fn build_embed(&self) -> Embed {
        self.clone()
    }
}

impl<T: Embedable + ?Sized> Embedable for &T {
    fn build_embed(&self) -> Embed {
        (*self).build_embed()
    }
}
