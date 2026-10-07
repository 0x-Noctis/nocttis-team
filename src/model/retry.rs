//! Kebijakan retry untuk panggilan model (M5-005): backoff eksponensial yang SELALU terbatas.
//!
//! Hanya kegagalan sementara (`RateLimited`, `Timeout`, `ProviderUnavailable`) yang boleh diulang
//! (`ModelError::retryable`). Autentikasi, respons tak valid, konteks terlalu besar, dan budget tidak pernah diulang:
//! mengulangnya tidak mengubah hasil dan hanya membakar token atau menutupi salah konfigurasi.

#![allow(dead_code)]

use std::{future::Future, time::Duration};

/// Batas keras jumlah percobaan dan jeda, apa pun isi konfigurasi.
pub const HARD_MAX_ATTEMPTS: u32 = 6;
pub const HARD_MAX_DELAY: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    /// Percobaan total per model termasuk yang pertama (1 = tanpa retry).
    max_attempts: u32,
    base_delay: Duration,
    max_delay: Duration,
    /// Jumlah maksimum waktu tidur untuk satu model sebelum menyerah ke fallback.
    max_total_delay: Duration,
}

impl RetryPolicy {
    /// Nilai di luar batas dipotong ke batas keras, bukan ditolak, supaya konfigurasi salah tidak bisa membuat loop liar.
    pub fn new(
        max_attempts: u32,
        base_delay: Duration,
        max_delay: Duration,
        max_total_delay: Duration,
    ) -> Self {
        Self {
            max_attempts: max_attempts.clamp(1, HARD_MAX_ATTEMPTS),
            base_delay: base_delay.min(HARD_MAX_DELAY),
            max_delay: max_delay.min(HARD_MAX_DELAY),
            max_total_delay: max_total_delay.min(HARD_MAX_DELAY * 2),
        }
    }

    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    pub fn max_total_delay(&self) -> Duration {
        self.max_total_delay
    }

    /// Jeda sebelum retry ke-`retry_number` (1 = retry pertama): `base * 2^(n-1)`, dipotong `max_delay`.
    pub fn delay_before(&self, retry_number: u32) -> Duration {
        let factor = 1u32
            .checked_shl(retry_number.saturating_sub(1).min(16))
            .unwrap_or(u32::MAX);
        self.base_delay
            .checked_mul(factor)
            .unwrap_or(self.max_delay)
            .min(self.max_delay)
    }
}

impl Default for RetryPolicy {
    /// 3 percobaan, jeda 500 ms lalu 1 s, total tidur paling lama 8 detik per model.
    fn default() -> Self {
        Self::new(
            3,
            Duration::from_millis(500),
            Duration::from_secs(8),
            Duration::from_secs(8),
        )
    }
}

/// Abstraksi tidur supaya jeda bisa diuji dengan jam palsu (tanpa menunggu waktu nyata).
pub trait Sleeper {
    fn sleep(&mut self, duration: Duration) -> impl Future<Output = ()>;
}

pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    async fn sleep(&mut self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}
