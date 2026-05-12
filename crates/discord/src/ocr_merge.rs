//! Spatial merge for multi-engine OCR output.
//!
//! When both Latin and CJK engines are configured, running the same image
//! through both produces two overlapping result sets — and showing the
//! user one block per engine is confusing because the same text often
//! lands in both (CJK engines typically read ASCII too; Latin engines
//! garble CJK characters but still emit something). This module flattens
//! everything into one list of `OcrLine`s, deduplicates spatially via
//! greedy NMS-by-IoU, and orders by reading position (top → left).
//!
//! Confidence-driven dedup: when two boxes from different engines cover
//! roughly the same region (IoU > [`MERGE_IOU_THRESHOLD`]), the result
//! with the higher OCR confidence wins. This naturally lets the
//! script-appropriate engine claim each text patch — Latin engine for
//! Latin text, CJK for CJK — without needing a hard-coded language pref.

use image::DynamicImage;
use ocr_rs::OcrEngine;
use tracing::warn;

use tomo_core::error::{Error, Result};

use crate::state::OcrSlot;

/// Two boxes are considered "the same region" when their IoU exceeds this.
/// 0.5 is the standard NMS threshold — strict enough that distinct stacked
/// lines stay distinct, loose enough that the two engines' jittery boxes
/// over the same word merge.
const MERGE_IOU_THRESHOLD: f32 = 0.5;

/// Vertical jitter tolerance (in px) for considering two boxes to be on
/// the same row when ordering output. Anything within this gets sorted
/// by `left` instead of `top` so a single visual line reads left-to-right
/// even when the engine emitted slightly mis-aligned tops.
const ROW_TOLERANCE: i32 = 8;

/// One merged line of recognised text, post-dedup. The bounding box is
/// stored as plain numbers (no `imageproc` dep on this crate) — `left`
/// and `top` are inclusive, `width`/`height` make up the size.
#[derive(Debug, Clone)]
pub struct OcrLine {
    pub text: String,
    pub confidence: f32,
    pub engine: &'static str,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
}

/// Run every engine and return one deduplicated, reading-order-sorted
/// list of lines. Empty when no engine matched anything.
pub fn run_merged(engines: &[OcrSlot], bytes: &[u8]) -> Result<Vec<OcrLine>> {
    let image = image::load_from_memory(bytes)
        .map_err(|e| Error::config(format!("ocr image load: {e}")))?;

    let mut all: Vec<OcrLine> = Vec::new();
    for slot in engines {
        match run_one(&slot.engine, &image, slot.name) {
            Ok(lines) => all.extend(lines),
            Err(e) => warn!(engine = slot.name, error = %e, "ocr engine failed"),
        }
    }
    Ok(merge(all))
}

/// Greedy NMS by IoU: iterate lines in descending confidence, keep each
/// only if it doesn't overlap (above the threshold) with anything already
/// kept. Then re-sort the survivors by reading order (top, then left)
/// so the joined output reads naturally.
fn merge(mut lines: Vec<OcrLine>) -> Vec<OcrLine> {
    if lines.len() <= 1 {
        return lines;
    }
    lines.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut kept: Vec<OcrLine> = Vec::with_capacity(lines.len());
    for line in lines {
        let collides = kept.iter().any(|k| iou(&line, k) > MERGE_IOU_THRESHOLD);
        if !collides {
            kept.push(line);
        }
    }

    kept.sort_by(|a, b| {
        if (a.top - b.top).abs() <= ROW_TOLERANCE {
            a.left.cmp(&b.left)
        } else {
            a.top.cmp(&b.top)
        }
    });
    kept
}

fn run_one(engine: &OcrEngine, image: &DynamicImage, name: &'static str) -> Result<Vec<OcrLine>> {
    let results = engine
        .recognize(image)
        .map_err(|e| Error::config(format!("ocr recognize: {e}")))?;
    Ok(results
        .into_iter()
        .map(|r| OcrLine {
            text: r.text,
            confidence: r.confidence,
            engine: name,
            left: r.bbox.rect.left(),
            top: r.bbox.rect.top(),
            width: r.bbox.rect.width(),
            height: r.bbox.rect.height(),
        })
        .filter(|l| !l.text.trim().is_empty())
        .collect())
}

/// Intersection-over-Union for two axis-aligned rectangles. Returns 0 for
/// non-overlapping or zero-area boxes (the `union <= 0` guard catches the
/// degenerate case).
fn iou(a: &OcrLine, b: &OcrLine) -> f32 {
    let a_right = a.left + a.width as i32;
    let a_bot = a.top + a.height as i32;
    let b_right = b.left + b.width as i32;
    let b_bot = b.top + b.height as i32;

    let ix1 = a.left.max(b.left);
    let iy1 = a.top.max(b.top);
    let ix2 = a_right.min(b_right);
    let iy2 = a_bot.min(b_bot);
    if ix2 <= ix1 || iy2 <= iy1 {
        return 0.0;
    }
    let inter = ((ix2 - ix1) as f32) * ((iy2 - iy1) as f32);
    let a_area = (a.width as f32) * (a.height as f32);
    let b_area = (b.width as f32) * (b.height as f32);
    let union = a_area + b_area - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Join merged lines into a single string with `\n` between them — what
/// the user actually sees in the `/ocr` embed and the LLM context.
pub fn render(lines: &[OcrLine]) -> String {
    let mut out = String::new();
    for line in lines {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&line.text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, confidence: f32, left: i32, top: i32, w: u32, h: u32) -> OcrLine {
        OcrLine {
            text: text.into(),
            confidence,
            engine: "test",
            left,
            top,
            width: w,
            height: h,
        }
    }

    #[test]
    fn iou_disjoint_is_zero() {
        let a = line("a", 0.9, 0, 0, 10, 10);
        let b = line("b", 0.9, 20, 20, 10, 10);
        assert!((iou(&a, &b) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn iou_identical_is_one() {
        let a = line("a", 0.9, 5, 5, 10, 10);
        let b = line("b", 0.9, 5, 5, 10, 10);
        assert!((iou(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn iou_half_overlap_is_one_third() {
        let a = line("a", 0.9, 0, 0, 10, 10);
        let b = line("b", 0.9, 5, 0, 10, 10);
        // Intersection 5x10 = 50; union 10x10 + 10x10 - 50 = 150; iou = 1/3.
        assert!((iou(&a, &b) - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn merge_dedups_overlapping_boxes_keeping_higher_confidence() {
        let lines = vec![
            line("garbled latin attempt", 0.30, 0, 0, 100, 20),
            line("こんにちは", 0.95, 2, 1, 98, 19), // IoU > 0.5 with above; better.
            line("Bottom row", 0.80, 0, 50, 100, 20),
        ];
        let merged = merge(lines);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].text, "こんにちは");
        assert_eq!(merged[1].text, "Bottom row");
    }

    #[test]
    fn merge_keeps_distinct_lines() {
        let lines = vec![
            line("Top", 0.9, 0, 0, 100, 20),
            line("Middle", 0.9, 0, 30, 100, 20),
            line("Bottom", 0.9, 0, 60, 100, 20),
        ];
        let merged = merge(lines);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].text, "Top");
        assert_eq!(merged[1].text, "Middle");
        assert_eq!(merged[2].text, "Bottom");
    }

    #[test]
    fn merge_sorts_same_row_left_to_right() {
        let lines = vec![
            line("RIGHT", 0.9, 50, 10, 40, 20),
            line("LEFT", 0.9, 0, 12, 40, 20), // small top jitter — same row.
        ];
        let merged = merge(lines);
        assert_eq!(merged[0].text, "LEFT");
        assert_eq!(merged[1].text, "RIGHT");
    }

    #[test]
    fn render_joins_with_newlines() {
        let lines = vec![
            line("a", 0.9, 0, 0, 10, 10),
            line("b", 0.9, 0, 30, 10, 10),
        ];
        assert_eq!(render(&lines), "a\nb");
    }
}
