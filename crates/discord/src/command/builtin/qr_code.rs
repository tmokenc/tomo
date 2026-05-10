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

use crate::command::{Command, CommandContext, CommandMeta};
use crate::util::fetch_image_from_context;

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
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let text = ctx.args().trim();

        if !text.is_empty() {
            return generate(&ctx, text.to_owned()).await;
        }

        let bytes = match fetch_image_from_context(&ctx).await {
            Ok(Some(b)) => b,
            Ok(None) => {
                return ctx
                    .reply(
                        "Give me text to encode, or attach (or reply to) an image \
                         containing a QR code.",
                    )
                    .await
            }
            Err(e) => return ctx.reply(&format!("Couldn't fetch image: `{e}`")).await,
        };

        let bytes_vec = bytes.to_vec();
        let decoded = task::spawn_blocking(move || decode_blocking(&bytes_vec))
            .await
            .map_err(|e| Error::config(format!("qr decode join: {e}")))??;

        if decoded.is_empty() {
            return ctx.reply("No QR codes I could read in that image.").await;
        }

        let body = decoded
            .iter()
            .map(|s| format!("```\n{s}\n```"))
            .collect::<Vec<_>>()
            .join("\n");
        ctx.reply(&body).await
    }
}

// ---------- Generation ----------

async fn generate(ctx: &CommandContext, text: String) -> Result<()> {
    if text.len() > 2_000 {
        return ctx
            .reply("Text is too long — QR codes get huge past a couple hundred bytes.")
            .await;
    }

    let png: Vec<u8> = task::spawn_blocking(move || render_png(&text))
        .await
        .map_err(|e| Error::config(format!("qrcode join: {e}")))??;

    let attachment = HttpAttachment::from_bytes("tomo_qrcode.png".into(), png, 0);
    ctx.bot
        .http
        .create_message(ctx.channel_id())
        .attachments(std::slice::from_ref(&attachment))
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
