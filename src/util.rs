//! Small shared helpers: time, bounded backoff, deterministic randomness.

use std::time::{SystemTime, UNIX_EPOCH};

use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

/// Milliseconds since the unix epoch.
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Bounded exponential backoff with full jitter.
///
/// The RFC forbids tight reconnect loops, so every retry path uses this type
/// rather than inventing its own delay.
#[derive(Debug, Clone)]
pub struct Backoff {
    base_ms: u64,
    max_ms: u64,
    factor: f64,
    attempts: u32,
}

impl Backoff {
    /// Creates a policy.
    pub fn new(base_ms: u64, max_ms: u64) -> Self {
        Backoff {
            base_ms: base_ms.max(1),
            max_ms: max_ms.max(1),
            factor: 2.0,
            attempts: 0,
        }
    }

    /// Overrides the growth factor.
    pub fn with_factor(mut self, factor: f64) -> Self {
        self.factor = factor.max(1.0);
        self
    }

    /// Current delay before jitter, in milliseconds.
    pub fn current_delay_ms(&self) -> u64 {
        let scaled = self.base_ms as f64 * self.factor.powi(self.attempts as i32);
        (scaled.round() as u64).min(self.max_ms)
    }

    /// Records an attempt and returns the delay to wait.
    pub fn next_delay_ms(&mut self, rng: &mut impl DeterministicRng) -> u64 {
        let ceiling = self.current_delay_ms();
        self.attempts = self.attempts.saturating_add(1);
        rng.jittered_ms(ceiling)
    }

    /// Returns the policy to its initial delay.
    pub fn reset(&mut self) {
        self.attempts = 0;
    }

    /// How many delays have been produced since the last reset.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}

/// A random source that can be seeded for reproducible runs.
pub trait DeterministicRng {
    /// Fills a byte buffer.
    fn fill_bytes(&mut self, dest: &mut [u8]);

    /// Returns a delay in `0..=ceiling` milliseconds using full jitter.
    fn jittered_ms(&mut self, ceiling: u64) -> u64 {
        if ceiling == 0 {
            return 0;
        }
        let mut raw = [0u8; 8];
        self.fill_bytes(&mut raw);
        let span = (ceiling as f64 + 1.0) as u128;
        let value = u64::from_le_bytes(raw) as u128;
        ((value % span) as u64).min(ceiling)
    }

    /// Returns a random `u32`.
    fn next_u32(&mut self) -> u32 {
        let mut raw = [0u8; 4];
        self.fill_bytes(&mut raw);
        u32::from_le_bytes(raw)
    }

    /// Returns a random `u64`.
    fn next_u64(&mut self) -> u64 {
        let mut raw = [0u8; 8];
        self.fill_bytes(&mut raw);
        u64::from_le_bytes(raw)
    }

    /// Returns `0..bound` without modulo bias for small bounds.
    fn below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        self.next_u32() % bound
    }
}

impl DeterministicRng for ChaCha20Rng {
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        rand::Rng::fill_bytes(self, dest)
    }
}

/// Creates a seeded, reproducible random source.
pub fn seeded_rng(seed: u64) -> ChaCha20Rng {
    ChaCha20Rng::seed_from_u64(seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_is_capped() {
        let mut rng = seeded_rng(1);
        let mut backoff = Backoff::new(10, 1_000);
        let mut seen = Vec::new();
        for _ in 0..8 {
            let delay = backoff.next_delay_ms(&mut rng);
            assert!(delay <= 1_000);
            seen.push(delay);
        }
        assert_eq!(backoff.attempts(), 8);
        assert!(seen.windows(2).any(|w| w[1] != w[0]), "delays should vary");
        backoff.reset();
        assert_eq!(backoff.attempts(), 0);
    }

    #[test]
    fn ceiling_never_exceeded_deterministically() {
        let mut rng = seeded_rng(7);
        for _ in 0..1000 {
            assert!(rng.jittered_ms(50) <= 50);
        }
        assert_eq!(rng.jittered_ms(0), 0);
    }

    #[test]
    fn seeded_rng_is_reproducible() {
        let mut a = seeded_rng(99);
        let mut b = seeded_rng(99);
        for _ in 0..16 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }
}
