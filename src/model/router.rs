//! Router model (M5-005): retry terbatas pada model utama lalu fallback ke model lain yang kompatibel.
//!
//! Aturan keselamatan (diuji di `tests/model_fallback.rs`):
//! - Hanya kegagalan sementara yang diulang; kegagalan lain dikembalikan seketika dan TIDAK memicu fallback
//!   (auth yang salah atau konteks yang terlalu besar adalah masalah konfigurasi/permintaan, bukan gangguan provider).
//! - Sebelum setiap retry/fallback, `CallGuard::side_effects_safe` memastikan tidak ada efek samping tool yang
//!   ambigu (reservasi tool yang belum selesai). Bila ada: berhenti dan kembalikan error aslinya.
//! - Sebelum setiap panggilan, `CallGuard::budget_allows`; budget habis => `BudgetExceeded`, tanpa retry.
//! - Fallback hanya ke model dengan class yang sama, tools terverifikasi bila permintaan memakai tools, dan context
//!   window yang cukup. Model utama (indeks 0) selalu dicoba lebih dulu apa adanya.

#![allow(dead_code)]

use std::future::Future;

use super::{
    ModelError, ModelErrorKind, ModelRequest, ModelResponse,
    retry::{RetryPolicy, Sleeper},
};

/// Satu model yang dapat dipanggil. Dipisah dari `WorkerModel` agar modul ini tidak bergantung pada lapisan agen.
pub trait CompletionModel {
    fn complete(
        &mut self,
        request: &ModelRequest,
    ) -> impl Future<Output = Result<ModelResponse, ModelError>>;
}

/// Pengaman sebelum mengulang atau berpindah model. Implementasi harus gagal TERTUTUP: bila tidak yakin, `false`.
pub trait CallGuard {
    fn side_effects_safe(&self, agent_run_id: &str) -> impl Future<Output = bool>;
    fn budget_allows(&self, request: &ModelRequest) -> impl Future<Output = bool>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelProfile {
    pub id: String,
    pub class: String,
    pub tools_verified: bool,
    pub context_window: u64,
}

pub struct Candidate<M> {
    pub profile: ModelProfile,
    pub model: M,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Step {
    Succeeded,
    /// Gagal sementara; menunggu lalu mengulang model yang sama.
    Retried(ModelErrorKind),
    /// Percobaan model ini habis; pindah ke model berikutnya.
    FellBack(ModelErrorKind),
    /// Dihentikan tanpa retry/fallback.
    Stopped(StopReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopReason {
    NotRetryable(ModelErrorKind),
    AmbiguousSideEffect,
    Budget,
    NoCandidate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attempt {
    pub model_id: String,
    pub number: u32,
    pub step: Step,
}

pub struct ModelRouter<M, S, G> {
    candidates: Vec<Candidate<M>>,
    policy: RetryPolicy,
    sleeper: S,
    guard: G,
    report: Vec<Attempt>,
}

impl<M: CompletionModel, S: Sleeper, G: CallGuard> ModelRouter<M, S, G> {
    /// `candidates[0]` adalah model utama; sisanya fallback berurutan.
    pub fn new(candidates: Vec<Candidate<M>>, policy: RetryPolicy, sleeper: S, guard: G) -> Self {
        Self {
            candidates,
            policy,
            sleeper,
            guard,
            report: Vec::new(),
        }
    }

    /// Jejak keputusan panggilan terakhir (untuk audit/observabilitas); mengosongkan catatan.
    pub fn take_report(&mut self) -> Vec<Attempt> {
        std::mem::take(&mut self.report)
    }

    fn eligible(&self, index: usize, request: &ModelRequest) -> bool {
        if index == 0 {
            return true;
        }
        let profile = &self.candidates[index].profile;
        profile.class == request.model_class
            && (request.tools.is_empty() || profile.tools_verified)
            && request.limits.max_input_tokens <= profile.context_window
    }

    pub async fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.report.clear();
        let order: Vec<usize> = (0..self.candidates.len())
            .filter(|index| self.eligible(*index, request))
            .collect();
        let mut last_error = ModelError::new(ModelErrorKind::ProviderUnavailable);
        let mut stopped = None;
        'models: for (position, index) in order.iter().copied().enumerate() {
            let id = self.candidates[index].profile.id.clone();
            let mut slept = std::time::Duration::ZERO;
            for number in 1..=self.policy.max_attempts() {
                if !self.guard.budget_allows(request).await {
                    self.note(&id, number, Step::Stopped(StopReason::Budget));
                    return Err(ModelError::new(ModelErrorKind::BudgetExceeded));
                }
                match self.candidates[index].model.complete(request).await {
                    Ok(response) => {
                        self.note(&id, number, Step::Succeeded);
                        return Ok(response);
                    }
                    Err(error) if !error.retryable() => {
                        self.note(
                            &id,
                            number,
                            Step::Stopped(StopReason::NotRetryable(error.kind())),
                        );
                        return Err(error);
                    }
                    Err(error) => {
                        let kind = error.kind();
                        last_error = error;
                        // Efek samping ambigu menutup kemungkinan retry maupun fallback.
                        if !self.guard.side_effects_safe(&request.agent_run_id).await {
                            stopped = Some(StopReason::AmbiguousSideEffect);
                            self.note(&id, number, Step::Stopped(StopReason::AmbiguousSideEffect));
                            break 'models;
                        }
                        let delay = self.policy.delay_before(number);
                        let can_retry = number < self.policy.max_attempts()
                            && slept + delay <= self.policy.max_total_delay();
                        if can_retry {
                            self.note(&id, number, Step::Retried(kind));
                            self.sleeper.sleep(delay).await;
                            slept += delay;
                        } else if position + 1 < order.len() {
                            self.note(&id, number, Step::FellBack(kind));
                            continue 'models;
                        } else {
                            self.note(&id, number, Step::Stopped(StopReason::NoCandidate));
                            break 'models;
                        }
                    }
                }
            }
        }
        let _ = stopped;
        Err(last_error)
    }

    fn note(&mut self, model_id: &str, number: u32, step: Step) {
        self.report.push(Attempt {
            model_id: model_id.to_owned(),
            number,
            step,
        });
    }
}
