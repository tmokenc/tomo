use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::TimeZone;
use chrono_tz::Tz;
use rhai::{Dynamic, Engine, OptimizationLevel};

use crate::ctx::ScriptCtx;

/// Render a chrono `DelayedFormat` without the panic `to_string()` would
/// raise on a bad pattern, returning `""` instead.
///
/// Scripts supply `strftime` patterns verbatim and are hot-reloaded operator
/// content, so a typo like `%Q` (unknown) or `%#z` (parse-only, no Display
/// impl) must not abort the process — release builds use `panic = "abort"`.
/// chrono's `Display` impl *returns* `Err` for these; only `ToString` turns
/// that into a panic via `.expect(...)`. Going through `write!` lets us catch
/// the `Err` directly, which covers every bad specifier (not just the
/// `Item::Error` subset a pre-parse check would find).
pub(crate) fn safe_strftime(formatted: impl std::fmt::Display) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if write!(out, "{formatted}").is_err() {
        return String::new();
    }
    out
}

/// Build a fresh Rhai engine wired up with everything scripts in this project
/// can touch.
pub fn make_engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_optimization_level(OptimizationLevel::Simple);
    engine.set_max_expr_depths(64, 32);
    engine.set_max_string_size(1 << 20); // 1 MiB
    engine.set_max_array_size(4096);
    engine.set_max_map_size(2048);
    engine.set_max_call_levels(32);
    engine.set_max_operations(1_000_000);

    engine.build_type::<ScriptCtx>();
    engine.build_type::<crate::embed::ScriptEmbed>();

    // Embed factories — scripts call any of `embed()`, `embed_info()`,
    // `embed_success()`, `embed_error()`, `embed_warning()`, `embed_lovely()`
    // to get a fresh builder pre-coloured for the situation.
    engine.register_fn("embed",         crate::embed::ScriptEmbed::new);
    engine.register_fn("embed_info",    crate::embed::ScriptEmbed::info);
    engine.register_fn("embed_success", crate::embed::ScriptEmbed::success);
    engine.register_fn("embed_error",   crate::embed::ScriptEmbed::error);
    engine.register_fn("embed_warning", crate::embed::ScriptEmbed::warning);
    engine.register_fn("embed_lovely",  crate::embed::ScriptEmbed::lovely);

    // Theme colours.
    engine.register_fn("color_info",    || 0x9966ffi64);
    engine.register_fn("color_success", || 0x3cb371i64);
    engine.register_fn("color_error",   || 0xff0033i64);
    engine.register_fn("color_warning", || 0xffaa00i64);
    engine.register_fn("color_lovely",  || 0xfc2368i64);

    // Time helpers.
    engine.register_fn("now_unix", || -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    });
    engine.register_fn("format_time", |unix: i64, fmt: &str| -> String {
        chrono::Utc
            .timestamp_opt(unix, 0)
            .single()
            .map(|t| safe_strftime(t.format(fmt)))
            .unwrap_or_default()
    });
    engine.register_fn("format_time_in", |tz: &str, unix: i64, fmt: &str| -> String {
        let parsed: std::result::Result<Tz, _> = tz.parse();
        let Ok(tz) = parsed else { return format!("(invalid tz: {tz})") };
        chrono::Utc
            .timestamp_opt(unix, 0)
            .single()
            .map(|t| safe_strftime(t.with_timezone(&tz).format(fmt)))
            .unwrap_or_default()
    });

    // GMT offset helper for the `/time` command's per-zone label —
    // returns "+07", "-05", "+00" (matches the tomoka-rs format).
    engine.register_fn("format_offset_in", |tz: &str, unix: i64| -> String {
        crate::myon::format_offset(tz, unix)
    });

    // Myon-time helpers — see `crate::myon`. When no `MYON_USER_ID` is
    // configured, `myon_enabled()` returns false and `myon_time()` returns
    // an empty string, so scripts can branch on the toggle.
    engine.register_fn("myon_enabled", || -> bool { crate::myon::is_enabled() });
    engine.register_fn("myon_label", || -> String {
        crate::myon::label().unwrap_or("").to_string()
    });
    engine.register_fn("myon_time", |unix: i64, fmt: &str| -> String {
        crate::myon::format_at(unix, fmt)
    });
    engine.register_fn("format_duration", |seconds: i64| -> String {
        if seconds <= 0 {
            return "0s".to_string();
        }
        humantime::format_duration(Duration::from_secs(seconds as u64)).to_string()
    });

    // Safe parse helpers. Rhai's built-in `parse_int` / `parse_float` RAISE
    // an error on bad input — the script aborts, the user sees nothing, the
    // bot logs `execute(): Error parsing integer number '...': invalid digit`.
    // These return `()` (unit) instead, so scripts can do
    // `if let n = try_parse_int(x) { ... }` or compare against `()` directly.
    engine.register_fn("try_parse_int", |s: &str| -> Dynamic {
        match s.trim().parse::<i64>() {
            Ok(n) => Dynamic::from_int(n),
            Err(_) => Dynamic::UNIT,
        }
    });
    engine.register_fn("try_parse_float", |s: &str| -> Dynamic {
        match s.trim().parse::<f64>() {
            Ok(n) => Dynamic::from_float(n),
            Err(_) => Dynamic::UNIT,
        }
    });

    // RNG.
    engine.register_fn("random_int", |min: i64, max: i64| -> i64 {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let span = (max - min).abs().max(1) as u64;
        min + ((seed % span) as i64)
    });
    engine.register_fn("random_float", || -> f64 {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        (seed % 1_000_000) as f64 / 1_000_000.0
    });

    // String helpers — `split("|")` and friends already exist in Rhai's stdlib.

    engine
}

#[cfg(test)]
mod tests {
    use super::safe_strftime;
    use chrono::TimeZone;

    fn fmt(pattern: &str) -> String {
        let t = chrono::Utc.timestamp_opt(0, 0).single().unwrap();
        safe_strftime(t.format(pattern))
    }

    #[test]
    fn valid_pattern_formats() {
        assert_eq!(fmt("%Y-%m-%d"), "1970-01-01");
    }

    #[test]
    fn unknown_specifier_does_not_panic() {
        // `%Q` is unknown → chrono's Display returns Err → we yield "".
        assert_eq!(fmt("%Q"), "");
    }

    #[test]
    fn parse_only_specifier_does_not_panic() {
        // `%#z` parses to a valid-but-unformattable item; `to_string()` would
        // panic here, but `safe_strftime` must just yield "".
        assert_eq!(fmt("%#z"), "");
    }
}
