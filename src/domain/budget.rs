//! Aturan budget hierarkis (M4-003): run -> task -> attempt -> request.
//!
//! Fungsi di sini murni (tanpa I/O) supaya aturannya bisa diuji tabel demi tabel; penyimpanan dan
//! penguncian ada di `store::budget`. Semua aritmetika memakai i128 agar tidak overflow di sekitar
//! `Number.MAX_SAFE_INTEGER`.

pub const WARNING_PERCENT: i128 = 70;
pub const CHECKPOINT_PERCENT: i128 = 85;
pub const STOP_PERCENT: i128 = 100;
/// Bagian budget run yang disisihkan untuk recovery dan integrasi; pekerjaan biasa tidak boleh memakainya.
pub const RESERVE_PERCENT: i128 = 15;

/// Tingkat pemakaian terhadap batas sebuah scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BudgetLevel {
    Ok,
    /// >= 70%: catat peringatan.
    Warning,
    /// >= 85%: buat checkpoint dan padatkan context.
    Checkpoint,
    /// >= 100%: berhenti, tidak ada request baru.
    Stop,
}

pub fn level(committed: i64, limit: i64) -> BudgetLevel {
    if limit <= 0 {
        return BudgetLevel::Stop;
    }
    let used = i128::from(committed.max(0)) * 100;
    let limit = i128::from(limit);
    if used >= limit * STOP_PERCENT {
        BudgetLevel::Stop
    } else if used >= limit * CHECKPOINT_PERCENT {
        BudgetLevel::Checkpoint
    } else if used >= limit * WARNING_PERCENT {
        BudgetLevel::Warning
    } else {
        BudgetLevel::Ok
    }
}

/// Reserve project, dibulatkan ke atas supaya budget kecil tetap menyisakan reserve.
pub fn project_reserve(limit: i64) -> i64 {
    let reserve = (i128::from(limit.max(0)) * RESERVE_PERCENT + 99) / 100;
    i64::try_from(reserve).unwrap_or(i64::MAX)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// Pekerjaan task biasa: terbatas pada budget run dikurangi reserve.
    Work,
    /// Recovery/integrasi: boleh memakai reserve sampai batas penuh budget run.
    Recovery,
}

/// Batas dan pemakaian satu scope. `committed` = usage tercatat + reservasi yang masih ditahan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    pub limit: i64,
    pub committed: i64,
}

impl Scope {
    pub fn remaining(self) -> i64 {
        self.limit.saturating_sub(self.committed).max(0)
    }
}

/// Snapshot yang dibutuhkan untuk memutuskan satu request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    /// Budget run penuh (`token_budget`); reserve dihitung di `decide` sesuai `Purpose`.
    pub run: Scope,
    /// Total semua attempt task: (max_input + max_output) x max_attempts.
    pub task: Scope,
    /// Input kumulatif attempt terhadap `max_input_tokens`.
    pub attempt_input: Scope,
    /// Output kumulatif attempt terhadap `max_output_tokens`.
    pub attempt_output: Scope,
}

/// Scope pertama yang menolak request, dengan sisa token di scope itu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denial {
    Run { remaining: i64 },
    Task { remaining: i64 },
    AttemptInput { remaining: i64 },
    AttemptOutput { remaining: i64 },
}

/// Tingkat pemakaian setelah request diterima; dipakai worker untuk checkpoint/stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Levels {
    pub run: BudgetLevel,
    pub task: BudgetLevel,
    pub attempt: BudgetLevel,
}

impl Levels {
    pub fn highest(self) -> BudgetLevel {
        self.run.max(self.task).max(self.attempt)
    }
}

/// Putuskan apakah request (estimasi input + batas output) boleh dikirim. Urutan pemeriksaan dari scope
/// terluar ke terdalam supaya penolakan menyebut batas yang paling mendasar.
pub fn decide(
    input: i64,
    output: i64,
    purpose: Purpose,
    position: &Position,
) -> Result<Levels, Denial> {
    let total = input.saturating_add(output);
    let run_limit = match purpose {
        Purpose::Work => position
            .run
            .limit
            .saturating_sub(project_reserve(position.run.limit)),
        Purpose::Recovery => position.run.limit,
    };
    let run = Scope {
        limit: run_limit,
        committed: position.run.committed,
    };
    if exceeds(run, total) {
        return Err(Denial::Run {
            remaining: run.remaining(),
        });
    }
    if exceeds(position.task, total) {
        return Err(Denial::Task {
            remaining: position.task.remaining(),
        });
    }
    if exceeds(position.attempt_input, input) {
        return Err(Denial::AttemptInput {
            remaining: position.attempt_input.remaining(),
        });
    }
    if exceeds(position.attempt_output, output) {
        return Err(Denial::AttemptOutput {
            remaining: position.attempt_output.remaining(),
        });
    }
    let after = |scope: Scope, add: i64| level(scope.committed.saturating_add(add), scope.limit);
    Ok(Levels {
        run: after(run, total),
        task: after(position.task, total),
        attempt: after(
            Scope {
                limit: position
                    .attempt_input
                    .limit
                    .saturating_add(position.attempt_output.limit),
                committed: position
                    .attempt_input
                    .committed
                    .saturating_add(position.attempt_output.committed),
            },
            total,
        ),
    })
}

/// Scope yang sudah penuh (>= 100%) menolak apa pun, termasuk request berukuran 0.
fn exceeds(scope: Scope, add: i64) -> bool {
    level(scope.committed, scope.limit) == BudgetLevel::Stop
        || scope.committed.saturating_add(add) > scope.limit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_switch_exactly_at_70_85_and_100_percent() {
        for (committed, expected) in [
            (0, BudgetLevel::Ok),
            (69, BudgetLevel::Ok),
            (70, BudgetLevel::Warning),
            (84, BudgetLevel::Warning),
            (85, BudgetLevel::Checkpoint),
            (99, BudgetLevel::Checkpoint),
            (100, BudgetLevel::Stop),
            (250, BudgetLevel::Stop),
            (-5, BudgetLevel::Ok),
        ] {
            assert_eq!(level(committed, 100), expected, "{committed}/100");
        }
        // Batas tidak valid dianggap penuh; angka sangat besar tidak overflow.
        assert_eq!(level(1, 0), BudgetLevel::Stop);
        assert_eq!(level(i64::MAX, i64::MAX), BudgetLevel::Stop);
        assert_eq!(level(i64::MAX / 2, i64::MAX), BudgetLevel::Ok);
    }

    #[test]
    fn reserve_rounds_up_and_never_exceeds_limit() {
        for (limit, reserve) in [(0, 0), (1, 1), (7, 2), (100, 15), (1_000, 150), (101, 16)] {
            assert_eq!(project_reserve(limit), reserve, "{limit}");
        }
        assert!(project_reserve(i64::MAX) < i64::MAX);
    }

    fn position(
        run: (i64, i64),
        task: (i64, i64),
        input: (i64, i64),
        output: (i64, i64),
    ) -> Position {
        let scope = |(limit, committed)| Scope { limit, committed };
        Position {
            run: scope(run),
            task: scope(task),
            attempt_input: scope(input),
            attempt_output: scope(output),
        }
    }

    #[test]
    fn work_cannot_touch_the_reserve_but_recovery_can() {
        let at_85 = position((1_000, 850), (10_000, 0), (10_000, 0), (10_000, 0));
        assert_eq!(
            decide(1, 0, Purpose::Work, &at_85),
            Err(Denial::Run { remaining: 0 })
        );
        let levels = decide(50, 50, Purpose::Recovery, &at_85).unwrap();
        assert_eq!(levels.run, BudgetLevel::Checkpoint); // 950/1000
        // Tepat 100% diterima, tetapi levelnya Stop sehingga request berikutnya ditolak.
        assert_eq!(
            decide(100, 50, Purpose::Recovery, &at_85).unwrap().run,
            BudgetLevel::Stop
        );
        // Reserve juga ada batasnya: total tidak boleh melewati budget run.
        assert_eq!(
            decide(
                1,
                0,
                Purpose::Recovery,
                &position((1_000, 1_000), (9, 0), (9, 0), (9, 0))
            ),
            Err(Denial::Run { remaining: 0 })
        );
        assert_eq!(
            decide(200, 0, Purpose::Recovery, &at_85),
            Err(Denial::Run { remaining: 150 })
        );
    }

    #[test]
    fn denial_names_the_innermost_scope_that_is_full() {
        let open = (1_000_000, 0);
        assert_eq!(
            decide(10, 5, Purpose::Work, &position(open, (14, 0), open, open)),
            Err(Denial::Task { remaining: 14 })
        );
        assert_eq!(
            decide(11, 1, Purpose::Work, &position(open, open, (10, 0), open)),
            Err(Denial::AttemptInput { remaining: 10 })
        );
        assert_eq!(
            decide(1, 6, Purpose::Work, &position(open, open, open, (5, 0))),
            Err(Denial::AttemptOutput { remaining: 5 })
        );
        // Run diperiksa lebih dulu daripada task/attempt.
        assert!(matches!(
            decide(
                1,
                1,
                Purpose::Work,
                &position((10, 9), (1, 1), (1, 1), (1, 1))
            ),
            Err(Denial::Run { .. })
        ));
    }

    #[test]
    fn full_scope_refuses_even_empty_requests() {
        let open = (1_000_000, 0);
        assert!(decide(0, 0, Purpose::Work, &position(open, (10, 10), open, open)).is_err());
        assert!(decide(0, 0, Purpose::Work, &position(open, (10, 9), open, open)).is_ok());
    }

    #[test]
    fn levels_reflect_usage_after_the_request() {
        let levels = decide(
            300,
            100,
            Purpose::Recovery,
            &position((1_000, 300), (1_000, 300), (800, 300), (200, 0)),
        )
        .unwrap();
        assert_eq!(levels.run, BudgetLevel::Warning); // 700/1000
        assert_eq!(levels.task, BudgetLevel::Warning);
        assert_eq!(levels.attempt, BudgetLevel::Warning); // (300 + 400)/1000
        assert_eq!(levels.highest(), BudgetLevel::Warning);
    }
}
