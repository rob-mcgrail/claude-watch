/// USD per million tokens (input, output) at the message's timestamp.
/// Cache read = 0.1x input, cache write 5m = 1.25x input, 1h = 2x input.
/// Verified against platform.claude.com/docs/en/about-claude/pricing, July 2026.
pub fn prices_usd_per_mtok(model: &str, ts_ms: i64) -> (f64, f64) {
    let m = model.to_ascii_lowercase();
    if m.contains("fable") || m.contains("mythos") {
        (10.0, 50.0)
    } else if m.contains("opus-4-1") || m.contains("opus-4-2025") {
        // legacy pricing: Opus 4 and 4.1 only
        (15.0, 75.0)
    } else if m.contains("opus") {
        // Opus 4.5 through Opus 5
        (5.0, 25.0)
    } else if m.contains("sonnet-5") {
        // introductory $2/$10 ends 2026-09-01T00:00:00Z
        const SONNET5_STANDARD_FROM: i64 = 1_788_220_800_000;
        if ts_ms >= SONNET5_STANDARD_FROM {
            (3.0, 15.0)
        } else {
            (2.0, 10.0)
        }
    } else if m.contains("sonnet") {
        (3.0, 15.0)
    } else if m.contains("haiku-4") {
        (1.0, 5.0)
    } else if m.contains("haiku") {
        (0.8, 4.0)
    } else {
        (3.0, 15.0)
    }
}

pub fn model_short(model: &str) -> &'static str {
    let m = model.to_ascii_lowercase();
    if m.contains("fable") {
        "fable"
    } else if m.contains("mythos") {
        "mythos"
    } else if m.contains("opus") {
        "opus"
    } else if m.contains("sonnet") {
        "son"
    } else if m.contains("haiku") {
        "haiku"
    } else {
        "?"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JUL_2026: i64 = 1_785_456_000_000;
    const OCT_2026: i64 = 1_790_812_800_000;

    #[test]
    fn prices_match_official_table() {
        assert_eq!(prices_usd_per_mtok("claude-fable-5", JUL_2026), (10.0, 50.0));
        assert_eq!(prices_usd_per_mtok("claude-opus-5", JUL_2026), (5.0, 25.0));
        // opus 4.5+ dropped to 5/25 — 4.8 must NOT hit legacy pricing
        assert_eq!(prices_usd_per_mtok("claude-opus-4-8", JUL_2026), (5.0, 25.0));
        assert_eq!(prices_usd_per_mtok("claude-opus-4-5-20251101", JUL_2026), (5.0, 25.0));
        // legacy models keep 15/75
        assert_eq!(prices_usd_per_mtok("claude-opus-4-1-20250805", JUL_2026), (15.0, 75.0));
        assert_eq!(prices_usd_per_mtok("claude-opus-4-20250514", JUL_2026), (15.0, 75.0));
        // sonnet 5 intro pricing ends 2026-09-01
        assert_eq!(prices_usd_per_mtok("claude-sonnet-5", JUL_2026), (2.0, 10.0));
        assert_eq!(prices_usd_per_mtok("claude-sonnet-5", OCT_2026), (3.0, 15.0));
        assert_eq!(prices_usd_per_mtok("claude-haiku-4-5-20251001", JUL_2026), (1.0, 5.0));
    }
}
