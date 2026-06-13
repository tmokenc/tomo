//! "Myon time" — a custom timezone for a specific tracked user.
//!
//! Ported from `tomoka-rs`'s time command. The premise: while the tracked
//! user is awake their day hasn't ended yet, so we display the wall-clock
//! hour shifted into the previous calendar day with `hour + 24`. Once
//! they sleep (no activity for a configured idle window *and* not in a
//! voice channel, or it's past the configured "by-now-they-must-have-slept"
//! hour) the display rolls over to the normal current day. Net effect:
//! their day can stretch to ~30:something hours before resetting.
//!
//! The tracker is process-global because it's updated from gateway events
//! (the discord crate) and read from Rhai scripts (this crate). Two
//! statics:
//!
//!   * [`CONFIG`] — set once at startup via [`init`]. Holds the tracked
//!     user id, base timezone, label, sleep threshold, idle window.
//!   * [`STATE`] — atomic counters mutated on every observed event.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use chrono::{TimeZone, Timelike};
use chrono_tz::Tz;

/// Configurable parameters. Populated once from env via [`init`]. While
/// `None` the tracker is dormant: [`is_enabled`] returns `false`,
/// [`format_at`] returns an empty string, and observations no-op.
pub struct MyonConfig {
    pub user_id: u64,
    pub timezone: Tz,
    pub label: String,
    /// Hour-of-day (in `timezone`) at or after which we always treat the
    /// tracked user as having slept, regardless of last_seen / voice
    /// state. Matches the original `tomoka-rs` `time.hour() > 6` check.
    pub sleep_hour: u32,
    /// How long with no observed activity counts as asleep, in seconds.
    /// Voice presence overrides this (in-voice = awake even if idle).
    pub idle_seconds: i64,
}

static CONFIG: OnceLock<MyonConfig> = OnceLock::new();

struct State {
    last_seen: AtomicI64,
    in_voice: AtomicBool,
}

static STATE: State = State {
    last_seen: AtomicI64::new(0),
    in_voice: AtomicBool::new(false),
};

/// Install the tracker. Called once during bot bootstrap. Subsequent
/// calls are silently ignored — the config is process-wide.
pub fn init(cfg: MyonConfig) {
    let _ = CONFIG.set(cfg);
}

pub fn is_enabled() -> bool {
    CONFIG.get().is_some()
}

pub fn label() -> Option<&'static str> {
    Some(CONFIG.get()?.label.as_str())
}

pub fn user_id() -> Option<u64> {
    Some(CONFIG.get()?.user_id)
}

/// Record an observed message from the tracked user. Other users are
/// ignored. `when_unix` is the Unix timestamp the activity was seen at.
pub fn observe_message(user_id: u64, when_unix: i64) {
    let Some(cfg) = CONFIG.get() else { return };
    if cfg.user_id != user_id {
        return;
    }
    STATE.last_seen.store(when_unix, Ordering::SeqCst);
}

/// Record a voice-state change for the tracked user. `in_voice = true`
/// means the user is currently connected to *some* voice channel; `false`
/// means they're disconnected.
pub fn observe_voice(user_id: u64, in_voice: bool, when_unix: i64) {
    let Some(cfg) = CONFIG.get() else { return };
    if cfg.user_id != user_id {
        return;
    }
    STATE.in_voice.store(in_voice, Ordering::SeqCst);
    // Joining / leaving voice also counts as activity — without this a
    // user who joins a voice channel and stays silent would be flagged as
    // asleep after `idle_seconds`.
    STATE.last_seen.store(when_unix, Ordering::SeqCst);
}

/// True iff the tracked user should be considered "already slept" at
/// `target_unix`. Mirrors `tomoka-rs::time::MyonTracking::slept`:
///   * if the wall-clock hour in `timezone` is past `sleep_hour`, yes;
///   * else if they're currently in voice, no;
///   * else if their last activity is more than `idle_seconds` before
///     `target_unix`, yes.
pub fn slept_at(target_unix: i64) -> bool {
    let Some(cfg) = CONFIG.get() else { return true };
    let local = match cfg.timezone.timestamp_opt(target_unix, 0).single() {
        Some(t) => t,
        None => return true,
    };
    if local.hour() > cfg.sleep_hour {
        return true;
    }
    if STATE.in_voice.load(Ordering::SeqCst) {
        return false;
    }
    let last_seen = STATE.last_seen.load(Ordering::SeqCst);
    target_unix.saturating_sub(last_seen) > cfg.idle_seconds
}

/// Format the Myon-style time string for `target_unix`. Returns an empty
/// string when the tracker isn't configured.
///
/// When `slept_at` says they're sleeping, the output is just `fmt` applied
/// to the local time. When they're awake, the output rolls the date back
/// one day and renders the hour as `hour + 24` so the "30-hour day"
/// effect kicks in. `fmt` is a chrono `strftime` pattern; the hour is
/// substituted in afterwards so any `%H` placeholders are replaced with
/// the shifted value.
pub fn format_at(target_unix: i64, fmt: &str) -> String {
    let Some(cfg) = CONFIG.get() else { return String::new() };
    let Some(local) = cfg.timezone.timestamp_opt(target_unix, 0).single() else {
        return String::new();
    };
    // `safe_strftime` swallows the panic an invalid specifier (e.g. `%Q`,
    // `%#z`) would otherwise raise in `to_string()` — see its docs.
    if slept_at(target_unix) {
        return crate::engine::safe_strftime(local.format(fmt));
    }
    // Awake — shift the rendering back one day, hour += 24.
    let shifted = local - chrono::Duration::days(1);
    let shifted_hour = shifted.hour() + 24;
    let rest = crate::engine::safe_strftime(shifted.format(strip_hour(fmt).as_str()));
    format!("{shifted_hour:02}:{rest}")
}

/// Compact GMT offset like `"+07"` for the timezone at `target_unix`.
/// Matches what `tomoka-rs` showed (`%:z` truncated to the first three
/// characters). Returns an empty string when the timezone is unparseable
/// (caller's bug — we don't want to leak panics into a chat command).
pub fn format_offset(tz: &str, target_unix: i64) -> String {
    let Ok(tz) = tz.parse::<Tz>() else { return String::new() };
    let Some(local) = tz.timestamp_opt(target_unix, 0).single() else {
        return String::new();
    };
    let full = local.format("%:z").to_string();
    full.chars().take(3).collect()
}

/// `fmt` minus its first `%H` token. Used by [`format_at`] when we render
/// the hour ourselves (with `+24`) and let the rest of the pattern stay.
/// Anything beyond a single leading `%H[:]` is left intact, so callers
/// who use `%H:%M %d/%m/%Y` get the expected `MM dd/mm/YYYY` tail.
fn strip_hour(fmt: &str) -> String {
    // Cheapest correct path: replace the first `%H` with empty and trim a
    // following `:` if it's the next char. Keeps the rest of the
    // pattern verbatim so user-supplied formats still work.
    if let Some(idx) = fmt.find("%H") {
        let mut out = String::with_capacity(fmt.len());
        out.push_str(&fmt[..idx]);
        let tail = &fmt[idx + 2..];
        let tail = tail.strip_prefix(':').unwrap_or(tail);
        out.push_str(tail);
        return out;
    }
    fmt.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(cfg: MyonConfig) {
        // Tests share the process-wide `OnceLock`, so we can't realistically
        // reset between runs. Install once; subsequent installs are silently
        // ignored. The tests below all use the same config so this is fine.
        init(cfg);
    }

    #[test]
    fn strip_hour_drops_leading_hh() {
        assert_eq!(strip_hour("%H:%M %d/%m/%Y"), "%M %d/%m/%Y");
        assert_eq!(strip_hour("%M %d/%m/%Y"), "%M %d/%m/%Y");
        assert_eq!(strip_hour("%H"), "");
    }

    #[test]
    fn slept_returns_true_when_no_config() {
        // Before init, the tracker should refuse to claim anyone's awake.
        // (Note: this only holds before `install` is called in another test
        // — once installed there's no de-init. We rely on test ordering
        // being indifferent: pre-install nothing else has set state.)
        let _ = CONFIG.get(); // touch
        assert!(slept_at(0));
    }

    #[test]
    fn slept_and_format_after_install() {
        install(MyonConfig {
            user_id: 1,
            timezone: chrono_tz::Asia::Ho_Chi_Minh,
            label: "Test".into(),
            sleep_hour: 6,
            idle_seconds: 300,
        });

        // 2024-01-01 02:00 ICT — early morning, not slept-by-hour, and
        // they've just been seen → awake.
        observe_message(1, 1_704_067_200);
        let early_morning_utc = 1_704_067_200; // 2024-01-01 00:00 UTC → 07:00 ICT (slept_hour=6 passes)
        // Hour 7 in ICT > 6 → asleep.
        assert!(slept_at(early_morning_utc));

        // Pre-dawn ICT, fresh activity → awake.
        let pre_dawn_utc = 1_704_067_200 - 6 * 3600; // 5 PM previous UTC = 00:00 ICT
        observe_message(1, pre_dawn_utc);
        assert!(!slept_at(pre_dawn_utc));

        // Older activity, not in voice → asleep.
        observe_message(1, pre_dawn_utc - 600);
        assert!(slept_at(pre_dawn_utc));

        // Old activity, but in voice → still awake.
        observe_voice(1, true, pre_dawn_utc - 600);
        // observe_voice also updates last_seen; back-date it manually to
        // confirm voice presence overrides idle timeout.
        STATE.last_seen.store(pre_dawn_utc - 3600, Ordering::SeqCst);
        assert!(!slept_at(pre_dawn_utc));
    }

    #[test]
    fn format_offset_returns_short_form() {
        assert_eq!(format_offset("Asia/Ho_Chi_Minh", 1_704_067_200), "+07");
        // Random UTC moment.
        assert_eq!(format_offset("UTC", 1_704_067_200), "+00");
        // Unparseable.
        assert_eq!(format_offset("Not/A/Tz", 1_704_067_200), "");
    }
}

