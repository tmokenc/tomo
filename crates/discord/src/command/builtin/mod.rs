//! Built-in Rust commands.
//!
//! Anything that needs richer integration with the bot internals (stats
//! queries, hot-reload, the requester crate, image work…) lives here. The
//! simple, IO-free commands (choose, random, uptime, time) ship as Rhai
//! scripts under `scripts/commands/` — that keeps the build fast and lets
//! operators hot-edit them without a recompile.

use std::sync::Arc;

use crate::command::Command;

mod booru;
mod ehentai;
mod gemini;
mod help;
mod info;
mod invite;
mod kanji;
mod nhentai;
mod ocr;
mod ping;
mod qr_code;
mod reload;
mod remind;
mod stats;
mod top;
mod urban;
mod vndb;

fn boxed<T: Command + 'static>(c: T) -> Arc<dyn Command> {
    Arc::new(c)
}

pub fn all() -> Vec<Arc<dyn Command>> {
    vec![
        // — General —
        boxed(ping::PingCommand),
        boxed(info::InfoCommand),
        boxed(invite::InviteCommand),
        boxed(help::HelpCommand),
        // — Utility —
        boxed(remind::RemindCommand),
        // — Search —
        boxed(urban::UrbanCommand),
        boxed(kanji::KanjiCommand),
        boxed(booru::BooruCommand),
        boxed(nhentai::NhentaiCommand),
        boxed(ehentai::EhentaiCommand),
        boxed(vndb::VndbCommand),
        // — Image —
        boxed(qr_code::QrCodeCommand),
        boxed(ocr::OcrCommand),
        // — Stats —
        boxed(stats::StatsCommand),
        boxed(top::TopCommand),
        // — Admin —
        boxed(reload::ReloadCommand),
        boxed(gemini::GeminiCommand),
    ]
}
