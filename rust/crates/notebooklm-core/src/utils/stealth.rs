//! Stealth utilities for human-like browser behavior
//! (Rust rewrite of src/utils/stealth-utils.ts)
//!
//! This module provides **pure math and timing** functions that can run and be
//! tested without a browser.  Browser-interaction functions (humanType,
//! randomMouseMovement, etc.) live in `browser_session.rs` (Phase 5) where a
//! `chromiumoxide::Page` is available.

use rand::Rng;
use rand_distr::{Distribution, Normal};

use crate::config::config;

// ---------------------------------------------------------------------------
// Random primitives
// ---------------------------------------------------------------------------

/// Random integer in [min, max] inclusive.
pub fn random_int(min: i64, max: i64) -> i64 {
    rand::thread_rng().gen_range(min..=max)
}

/// Random float in [min, max).
pub fn random_float(min: f64, max: f64) -> f64 {
    if min >= max {
        return min;
    }
    rand::thread_rng().gen_range(min..max)
}

/// Random lowercase keyboard character (for typo simulation).
pub fn random_char() -> char {
    const CHARS: &[u8] = b"qwertyuiopasdfghjklzxcvbnm";
    let idx = rand::thread_rng().gen_range(0..CHARS.len());
    CHARS[idx] as char
}

// ---------------------------------------------------------------------------
// Gaussian / Normal distribution
// ---------------------------------------------------------------------------

/// Sample from a Gaussian distribution using `rand_distr::Normal`.
///
/// Falls back to `mean` when `std_dev <= 0`.
pub fn gaussian(mean: f64, std_dev: f64) -> f64 {
    if std_dev <= 0.0 {
        return mean;
    }
    match Normal::new(mean, std_dev) {
        Ok(dist) => dist.sample(&mut rand::thread_rng()),
        Err(_) => mean,
    }
}

/// Compute a Gaussian-distributed delay clamped to `[min_ms, max_ms]`.
///
/// Matches TypeScript stealth-utils:
/// - mean    = min + range × 0.6
/// - std_dev = range × 0.2
pub fn gaussian_delay_ms(min_ms: f64, max_ms: f64) -> f64 {
    let range = max_ms - min_ms;
    if range <= 0.0 {
        return min_ms.max(0.0);
    }
    let mean = min_ms + range * 0.6;
    let std_dev = range * 0.2;
    gaussian(mean, std_dev).clamp(min_ms, max_ms)
}

// ---------------------------------------------------------------------------
// Async delay
// ---------------------------------------------------------------------------

/// Sleep for a Gaussian-distributed duration in `[min_ms, max_ms]`.
///
/// When stealth random-delays are disabled the midpoint is used instead,
/// matching the TypeScript `!CONFIG.stealthRandomDelays` branch.
pub async fn random_delay(min_ms: f64, max_ms: f64) {
    let cfg = config();
    let ms = if cfg.stealth_enabled && cfg.stealth_random_delays {
        gaussian_delay_ms(min_ms, max_ms)
    } else {
        if (min_ms - max_ms).abs() < f64::EPSILON {
            min_ms
        } else {
            (min_ms + max_ms) / 2.0
        }
    };
    if ms > 0.0 {
        tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
    }
}

// ---------------------------------------------------------------------------
// Typing timing
// ---------------------------------------------------------------------------

/// Convert WPM to average milliseconds per character.
///
/// Formula: `avg_ms = 60_000 / (wpm × 5)`
/// (Average word length ≈ 5 characters.)
pub fn wpm_to_avg_char_delay_ms(wpm: u32) -> f64 {
    if wpm == 0 {
        return 200.0;
    }
    60_000.0 / (wpm as f64 * 5.0)
}

/// Per-character delay variation matching TypeScript stealth-utils.
///
/// - End-of-sentence (`.!?`): 1.05–1.4× average
/// - Space:  0.5–0.9× average
/// - Comma:  0.9–1.2× average
/// - Other:  0.5–0.9× average (random variation)
pub fn char_type_delay_ms(ch: char, avg_ms: f64) -> f64 {
    match ch {
        '.' | '!' | '?' => random_float(avg_ms * 1.05, avg_ms * 1.4),
        ' ' => random_float(avg_ms * 0.5, avg_ms * 0.9),
        ',' => random_float(avg_ms * 0.9, avg_ms * 1.2),
        _ => {
            let variation = random_float(0.5, 0.9);
            avg_ms * variation
        }
    }
}

/// Returns the WPM to use for a typing operation.
///
/// Picks a random value in `[typing_wpm_min, typing_wpm_max]` from config,
/// unless `override_wpm` is supplied.
pub fn effective_wpm(override_wpm: Option<u32>) -> u32 {
    if let Some(w) = override_wpm {
        return w;
    }
    let cfg = config();
    random_int(cfg.typing_wpm_min as i64, cfg.typing_wpm_max as i64) as u32
}

// ---------------------------------------------------------------------------
// Reading simulation
// ---------------------------------------------------------------------------

/// Time (ms) a human would spend reading `text_len` characters at `wpm` WPM.
///
/// Adds ±20 % randomness and caps at 3 000 ms, matching TypeScript.
pub fn reading_pause_ms(text_len: usize, wpm: u32) -> f64 {
    if wpm == 0 || text_len == 0 {
        return 0.0;
    }
    let word_count = text_len as f64 / 5.0;
    let minutes = word_count / wpm as f64;
    let seconds = minutes * 60.0 * random_float(0.8, 1.2);
    (seconds * 1_000.0).min(3_000.0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Gaussian output should be in a sensible range around the mean.
    #[test]
    fn gaussian_roughly_centred() {
        let mut sum = 0.0_f64;
        let n = 10_000;
        for _ in 0..n {
            sum += gaussian(100.0, 10.0);
        }
        let mean = sum / n as f64;
        // Mean should be within ±1 of target with 10k samples
        assert!(
            (mean - 100.0).abs() < 1.0,
            "gaussian mean {mean:.2} too far from 100"
        );
    }

    /// gaussian_delay_ms should never return values outside [min, max].
    #[test]
    fn gaussian_delay_clamped() {
        for _ in 0..1_000 {
            let d = gaussian_delay_ms(200.0, 800.0);
            assert!(d >= 200.0 && d <= 800.0, "delay {d} out of [200, 800]");
        }
    }

    /// Edge: equal min/max should return min.
    #[test]
    fn gaussian_delay_equal_bounds() {
        let d = gaussian_delay_ms(500.0, 500.0);
        assert!((d - 500.0).abs() < f64::EPSILON);
    }

    /// WPM math sanity: 60 WPM → 200 ms/char.
    #[test]
    fn wpm_conversion_60wpm() {
        let ms = wpm_to_avg_char_delay_ms(60);
        // 60 WPM × 5 chars/word = 300 chars/min → 60_000/300 = 200 ms
        assert!((ms - 200.0).abs() < f64::EPSILON, "expected 200 ms, got {ms}");
    }

    /// WPM 0 returns the sane default without panic.
    #[test]
    fn wpm_zero_no_panic() {
        let ms = wpm_to_avg_char_delay_ms(0);
        assert!(ms > 0.0);
    }

    /// Reading pause is capped at 3 000 ms.
    #[test]
    fn reading_pause_cap() {
        // Very long text at slow speed → must cap
        let ms = reading_pause_ms(100_000, 10);
        assert!(ms <= 3_000.0, "reading_pause_ms uncapped: {ms}");
    }

    /// Reading pause is 0 for zero-length text.
    #[test]
    fn reading_pause_empty_text() {
        assert_eq!(reading_pause_ms(0, 200), 0.0);
    }

    /// random_float never panics when min == max.
    #[test]
    fn random_float_equal_bounds() {
        let v = random_float(42.0, 42.0);
        assert!((v - 42.0).abs() < f64::EPSILON);
    }

    /// char_type_delay_ms respects punctuation multipliers.
    #[test]
    fn char_delay_punctuation_longer() {
        let avg = 100.0;
        let dot_delay = char_type_delay_ms('.', avg);
        let normal_delay = char_type_delay_ms('a', avg);
        // Both should be positive; punctuation is in [105, 140] ms range
        assert!(dot_delay >= avg * 1.05 - 0.01 && dot_delay <= avg * 1.4 + 0.01);
        assert!(normal_delay >= avg * 0.5 - 0.01 && normal_delay <= avg * 0.9 + 0.01);
    }
}
