/// USD per million tokens (input, output). Cache read = 0.1x input,
/// cache write 5m = 1.25x input, cache write 1h = 2x input.
/// Rates as of July 2026.
pub fn prices_usd_per_mtok(model: &str) -> (f64, f64) {
    let m = model.to_ascii_lowercase();
    if m.contains("fable") {
        (10.0, 50.0)
    } else if m.contains("opus-5") || m.contains("opus-4-5") {
        (5.0, 25.0)
    } else if m.contains("opus") {
        (15.0, 75.0)
    } else if m.contains("sonnet-5") {
        (2.0, 10.0)
    } else if m.contains("sonnet") {
        (3.0, 15.0)
    } else if m.contains("haiku-4-5") {
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
