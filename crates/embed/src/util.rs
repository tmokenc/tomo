//! Display helpers shared by the builder and call-sites that want to keep
//! free-form text under Discord's limits.

/// Truncate a string to at most `max` Unicode scalar values, appending `…`
/// if anything was cut off. Operates on chars (not bytes), so it never
/// splits a codepoint.
///
/// `max == 0` returns an empty string.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
