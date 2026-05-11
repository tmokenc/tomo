//! `qr <text|nothing>` — mirrors tomoka-rs's behaviour:
//! * With text: encode it as a QR code and send the PNG.
//! * Without text: look for an image to decode — first on the invoking
//!   message, then on the message it replied to, then on the channel's
//!   recent cache. Returns whatever text the QR code carries.

use std::sync::LazyLock;

use async_trait::async_trait;
use image::{ImageFormat, Luma};
use qrcode::QrCode;
use tokio::task;
use tracing::warn;
use twilight_model::http::attachment::Attachment as HttpAttachment;

use tomo_core::error::{Error, Result};
use tomo_embed::{Embed2, Embedable};

use crate::command::{Command, CommandContext, CommandMeta};
use crate::util::{fetch_image_from_context, truncate};

const QR_FILENAME: &str = "tomo_qrcode.png";

pub struct QrCodeCommand;

#[async_trait]
impl Command for QrCodeCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new(
                "qr",
                "Generate or read a QR code. Provide text to encode; \
                 attach (or reply to) an image to decode.",
            )
            .aliases(["qrcode", "qr_code"])
            .category("Image")
            .string_option("text", "Text to encode (leave empty to decode an attached image)", false)
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let text = ctx.args().trim();
        if !text.is_empty() {
            return generate(&ctx, text.to_owned()).await;
        }
        decode(&ctx).await
    }
}

// ---------- Generation ----------

async fn generate(ctx: &CommandContext, text: String) -> Result<()> {
    if text.len() > 2_000 {
        let mut embed = Embed2::warning()
            .title("QR code")
            .description("Text is too long — QR codes get huge past a couple hundred bytes.")
            .field_inline("Bytes", text.len().to_string())
            .field_inline("Limit", "2000")
            .timestamp_now();
        if let Some(user) = ctx.author() {
            embed = embed.author_user(user);
        }
        return ctx.reply_embed(&embed).await;
    }

    let text_for_embed = text.clone();
    let png: Vec<u8> = task::spawn_blocking(move || render_png(&text))
        .await
        .map_err(|e| Error::config(format!("qrcode join: {e}")))??;

    let mut embed = Embed2::info()
        .title("QR code")
        .description(format!("```\n{}\n```", truncate(&text_for_embed, 1024)))
        .image_attachment(QR_FILENAME)
        .field_inline("Bytes", text_for_embed.len().to_string())
        .field_inline("Chars", text_for_embed.chars().count().to_string())
        .field_inline("PNG", format!("{} B", png.len()))
        .footer("Encoded with qrcode-rs · 256×256 min")
        .timestamp_now();
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }

    let attachment = HttpAttachment::from_bytes(QR_FILENAME.into(), png, 0);
    let built = embed.build_embed();
    ctx.bot
        .http
        .create_message(ctx.channel_id())
        .attachments(std::slice::from_ref(&attachment))
        .embeds(std::slice::from_ref(&built))
        .await
        .map_err(|e| Error::config(format!("qr send: {e}")))?;
    Ok(())
}

fn render_png(text: &str) -> Result<Vec<u8>> {
    let code = QrCode::new(text.as_bytes())
        .map_err(|e| Error::config(format!("qr encode: {e}")))?;
    let image = code
        .render::<Luma<u8>>()
        .min_dimensions(256, 256)
        .quiet_zone(true)
        .build();

    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut cursor = std::io::Cursor::new(&mut buf);
    image
        .write_to(&mut cursor, ImageFormat::Png)
        .map_err(|e| Error::config(format!("qr png: {e}")))?;
    Ok(buf)
}

// ---------- Decoding ----------

async fn decode(ctx: &CommandContext) -> Result<()> {
    let bytes = match fetch_image_from_context(ctx).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            let mut embed = Embed2::warning()
                .title("QR code")
                .description(
                    "Give me text to encode, or attach (or reply to) an image \
                     containing a QR code.",
                )
                .timestamp_now();
            if let Some(user) = ctx.author() {
                embed = embed.author_user(user);
            }
            return ctx.reply_embed(&embed).await;
        }
        Err(e) => {
            let mut embed = Embed2::error()
                .title("QR code")
                .description(format!("Couldn't fetch image: `{e}`"))
                .timestamp_now();
            if let Some(user) = ctx.author() {
                embed = embed.author_user(user);
            }
            return ctx.reply_embed(&embed).await;
        }
    };

    let bytes_vec = bytes.to_vec();
    let decoded = task::spawn_blocking(move || decode_blocking(&bytes_vec))
        .await
        .map_err(|e| Error::config(format!("qr decode join: {e}")))??;

    if decoded.is_empty() {
        let mut embed = Embed2::warning()
            .title("QR code")
            .description("No QR codes I could read in that image.")
            .timestamp_now();
        if let Some(user) = ctx.author() {
            embed = embed.author_user(user);
        }
        return ctx.reply_embed(&embed).await;
    }

    let total_chars: usize = decoded.iter().map(|s| s.chars().count()).sum();
    let mut embed = Embed2::success()
        .title(if decoded.len() == 1 {
            "QR code decoded".to_string()
        } else {
            format!("{} QR codes decoded", decoded.len())
        })
        .field_inline("Codes", decoded.len().to_string())
        .field_inline("Total chars", total_chars.to_string())
        .footer("Decoded with rqrr")
        .timestamp_now();

    if decoded.len() == 1 {
        embed = embed.description(format!("```\n{}\n```", truncate(&decoded[0], 3500)));
    } else {
        for (i, content) in decoded.iter().enumerate() {
            embed = embed.field_block(
                format!("#{}", i + 1),
                format!("```\n{}\n```", truncate(content, 1000)),
            );
        }
    }
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}

fn decode_blocking(bytes: &[u8]) -> Result<Vec<String>> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| Error::config(format!("qr image load: {e}")))?
        .into_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(img);
    let mut out = Vec::new();
    for grid in prepared.detect_grids() {
        match grid.decode() {
            Ok((_meta, content)) => out.push(content),
            Err(e) => warn!(error = %e, "qr grid decode failed"),
        }
    }
    Ok(out)
}
