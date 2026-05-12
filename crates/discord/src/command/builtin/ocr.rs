//! `ocr` — run OCR over an image and reply with the recognised text.
//!
//! Backed by `ocr-rs` (PaddleOCR / PP-OCRv5 via MNN). Zero, one, or two
//! engines may be configured at startup — see the `TOMO_OCR_*` env vars in
//! `.env.example`. With both `latin` and `cjk` configured the command runs
//! every engine and merges their bounding boxes spatially so the user
//! sees a single coherent block of text, not one block per engine.

use std::collections::BTreeSet;
use std::sync::LazyLock;
use std::time::Instant;

use async_trait::async_trait;
use tokio::task;
use tracing::warn;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};
use crate::ocr_merge::{self, OcrLine};
use crate::state::OcrSlot;
use crate::util::{fetch_image_from_context, truncate};

pub struct OcrCommand;

#[async_trait]
impl Command for OcrCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new(
                "ocr",
                "Extract text from an image (attach one, or reply to one).",
            )
            .category("Image")
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        if ctx.bot.ocr.is_empty() {
            return reply_warn(
                &ctx,
                "OCR not configured",
                "OCR isn't configured on this bot — the operator needs to point \
                 `TOMO_OCR_*` env vars at PaddleOCR model files.",
            )
            .await;
        }

        let bytes = match fetch_image_from_context(&ctx).await {
            Ok(Some(b)) => b,
            Ok(None) => {
                return reply_warn(
                    &ctx,
                    "OCR",
                    "Attach (or reply to) an image with text in it.",
                )
                .await
            }
            Err(e) => return reply_error(&ctx, &format!("Couldn't fetch the image: `{e}`")).await,
        };

        let _ = ctx.bot.http.create_typing_trigger(ctx.channel_id()).await;

        let engines: Vec<OcrSlot> = ctx.bot.ocr.clone();
        let bytes_for_blocking = bytes.clone();
        let started = Instant::now();

        let lines: Vec<OcrLine> =
            match task::spawn_blocking(move || ocr_merge::run_merged(&engines, &bytes_for_blocking))
                .await
            {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    warn!(error = %e, "ocr engine error");
                    return reply_error(&ctx, &format!("OCR failed: `{e}`")).await;
                }
                Err(e) => {
                    warn!(error = %e, "ocr task join failed");
                    return reply_error(&ctx, "OCR worker crashed — check the bot logs.").await;
                }
            };

        if lines.is_empty() {
            return reply_warn(&ctx, "OCR", "I couldn't find any text in that image.").await;
        }
        let elapsed = started.elapsed();

        let merged_text = ocr_merge::render(&lines);
        let total_chars: usize = merged_text.chars().count();
        // Deduped, sorted set of contributing engine names — preserves a stable
        // alphabetical order in the embed footer regardless of input ordering.
        let engine_names: BTreeSet<&'static str> = lines.iter().map(|l| l.engine).collect();
        let engine_names = engine_names.into_iter().collect::<Vec<_>>().join(", ");

        let mut embed = Embed2::info()
            .title("OCR result")
            .description(format!("```\n{}\n```", truncate(&merged_text, 3500)));

        embed = embed
            .field_inline("Engines", engine_names)
            .field_inline("Characters", total_chars.to_string())
            .field_inline("Duration", format!("{elapsed:?}"))
            .footer("Powered by PaddleOCR (ocr-rs)")
            .timestamp_now();
        if let Some(user) = ctx.author() {
            embed = embed.author_user(user);
        }
        ctx.reply_embed(&embed).await
    }
}

async fn reply_warn(ctx: &CommandContext, title: &str, message: &str) -> Result<()> {
    let mut embed = Embed2::warning()
        .title(title.to_string())
        .description(message.to_string())
        .timestamp_now();
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}

async fn reply_error(ctx: &CommandContext, message: &str) -> Result<()> {
    let mut embed = Embed2::error()
        .title("OCR")
        .description(message.to_string())
        .timestamp_now();
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}

