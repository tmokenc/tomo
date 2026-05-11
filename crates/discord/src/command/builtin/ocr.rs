//! `ocr` — run OCR over an image and reply with the recognised text.
//!
//! Backed by `ocr-rs` (PaddleOCR / PP-OCRv5 via MNN). Zero, one, or two
//! engines may be configured at startup — see the `TOMO_OCR_*` env vars in
//! `.env.example`. With both `latin` and `cjk` configured the command merges
//! the output of both, so a single image can contain mixed scripts.

use std::sync::LazyLock;
use std::time::Instant;

use async_trait::async_trait;
use ocr_rs::OcrEngine;
use tokio::task;
use tracing::warn;

use tomo_core::error::{Error, Result};
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};
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

        let mut sections: Vec<(String, String)> =
            match task::spawn_blocking(move || run_all(&engines, &bytes_for_blocking)).await {
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

        sections.retain(|(_, text)| !text.trim().is_empty());
        if sections.is_empty() {
            return reply_warn(&ctx, "OCR", "I couldn't find any text in that image.").await;
        }
        let elapsed = started.elapsed();

        let total_chars: usize = sections.iter().map(|(_, t)| t.chars().count()).sum();
        let engine_names = sections
            .iter()
            .map(|(n, _)| n.clone())
            .collect::<Vec<_>>()
            .join(", ");

        let mut embed = Embed2::info().title("OCR result");
        // If only one engine produced output, keep the description compact.
        if sections.len() == 1 {
            let (_, text) = sections.into_iter().next().unwrap();
            embed = embed.description(format!("```\n{}\n```", truncate(&text, 3500)));
        } else {
            embed = embed.description("Mixed-script content — output split per engine.");
            for (name, text) in sections {
                embed = embed.field_block(
                    format!("{name} engine"),
                    format!("```\n{}\n```", truncate(&text, 1000)),
                );
            }
        }

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

fn run_all(engines: &[OcrSlot], bytes: &[u8]) -> Result<Vec<(String, String)>> {
    let image = image::load_from_memory(bytes)
        .map_err(|e| Error::config(format!("ocr image load: {e}")))?;
    let mut out = Vec::new();
    for e in engines {
        let text = run_one(&e.engine, &image)?;
        out.push((e.name.to_string(), text));
    }
    Ok(out)
}

fn run_one(engine: &OcrEngine, image: &image::DynamicImage) -> Result<String> {
    let results = engine
        .recognize(image)
        .map_err(|e| Error::config(format!("ocr recognize: {e}")))?;
    let mut out = String::new();
    for r in results {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&r.text);
    }
    Ok(out)
}
