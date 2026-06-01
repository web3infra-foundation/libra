//! Auto-merge readiness metrics (CEX-S2-15, Step 2.5).
//!
//! CEX-S2-15 acceptance criterion (5): the experimental auto-merge flag may only
//! be turned on after "连续基准任务的冲突率、回滚率和验证通过率报告" shows the
//! candidate is stable — the doc pins the thresholds at `conflict_rate < 5%` and
//! `rollback_rate < 1%` (`AutoMergeConfig` doc), with a high validation pass
//! rate. This module is the **pure** aggregator that turns a window of merge
//! outcomes into those three rates and a readiness verdict.
//!
//! It only computes rates from counts — it does not collect the window (that is
//! the operator's 30-day fixture) and does not flip the flag (that stays with
//! the config / human). Pure; no I/O.

/// Counts collected over a window of merge candidates, fed to
/// [`MergeMetrics::from_counts`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MergeOutcomeCounts {
    /// Total merge candidates observed in the window.
    pub total: u64,
    /// Candidates that hit at least one conflict.
    pub conflicted: u64,
    /// Merged candidates that were later rolled back.
    pub rolled_back: u64,
    /// Candidates whose validation passed.
    pub validation_passed: u64,
}

/// The computed conflict / rollback / validation-pass rates plus the readiness
/// verdict for enabling auto-merge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MergeMetrics {
    /// `conflicted / total` in `0.0..=1.0`. `0.0` for an empty window.
    pub conflict_rate: f64,
    /// `rolled_back / total` in `0.0..=1.0`.
    pub rollback_rate: f64,
    /// `validation_passed / total` in `0.0..=1.0`.
    pub validation_pass_rate: f64,
    /// Number of candidates the rates were computed over.
    pub sample_size: u64,
}

/// The documented auto-merge readiness thresholds (`AutoMergeConfig` doc):
/// conflict rate below 5%, rollback rate below 1%.
pub const MAX_CONFLICT_RATE: f64 = 0.05;
pub const MAX_ROLLBACK_RATE: f64 = 0.01;
/// A high validation pass rate is also required before auto-merge; 95% is the
/// conservative companion to the conflict / rollback ceilings.
pub const MIN_VALIDATION_PASS_RATE: f64 = 0.95;
/// Auto-merge readiness needs a meaningful window; a handful of candidates can
/// hit 0% conflict by luck. Require a minimum sample before the gate can pass.
pub const MIN_SAMPLE_SIZE: u64 = 100;

impl MergeMetrics {
    /// Compute the rates from a window of outcome `counts`.
    ///
    /// An empty window yields all-zero rates (and never divides by zero); its
    /// readiness is always `false` via the [`MIN_SAMPLE_SIZE`] gate.
    pub fn from_counts(counts: &MergeOutcomeCounts) -> Self {
        let rate = |numerator: u64| {
            if counts.total == 0 {
                0.0
            } else {
                numerator as f64 / counts.total as f64
            }
        };
        Self {
            conflict_rate: rate(counts.conflicted),
            rollback_rate: rate(counts.rolled_back),
            validation_pass_rate: rate(counts.validation_passed),
            sample_size: counts.total,
        }
    }

    /// Whether the window clears every auto-merge readiness threshold: a large
    /// enough sample, conflict rate under [`MAX_CONFLICT_RATE`], rollback rate
    /// under [`MAX_ROLLBACK_RATE`], and validation pass rate at or above
    /// [`MIN_VALIDATION_PASS_RATE`]. This is the gate CEX-S2-15 criterion (5)
    /// requires *before* an operator may enable the auto-merge flag.
    pub fn auto_merge_ready(&self) -> bool {
        self.sample_size >= MIN_SAMPLE_SIZE
            && self.conflict_rate < MAX_CONFLICT_RATE
            && self.rollback_rate < MAX_ROLLBACK_RATE
            && self.validation_pass_rate >= MIN_VALIDATION_PASS_RATE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_window_is_zero_rates_and_not_ready() {
        let metrics = MergeMetrics::from_counts(&MergeOutcomeCounts::default());
        assert_eq!(metrics.conflict_rate, 0.0);
        assert_eq!(metrics.rollback_rate, 0.0);
        assert_eq!(metrics.validation_pass_rate, 0.0);
        assert_eq!(metrics.sample_size, 0);
        assert!(
            !metrics.auto_merge_ready(),
            "an empty window must never be auto-merge ready",
        );
    }

    #[test]
    fn rates_are_computed_as_fractions_of_total() {
        let metrics = MergeMetrics::from_counts(&MergeOutcomeCounts {
            total: 200,
            conflicted: 8,
            rolled_back: 1,
            validation_passed: 196,
        });
        assert!((metrics.conflict_rate - 0.04).abs() < 1e-9);
        assert!((metrics.rollback_rate - 0.005).abs() < 1e-9);
        assert!((metrics.validation_pass_rate - 0.98).abs() < 1e-9);
    }

    #[test]
    fn a_clean_large_window_is_ready() {
        // 200 candidates: 4% conflict, 0.5% rollback, 98% validation — all clear.
        let metrics = MergeMetrics::from_counts(&MergeOutcomeCounts {
            total: 200,
            conflicted: 8,
            rolled_back: 1,
            validation_passed: 196,
        });
        assert!(metrics.auto_merge_ready());
    }

    #[test]
    fn too_small_a_sample_is_never_ready() {
        // Perfect rates but only 50 candidates — below MIN_SAMPLE_SIZE.
        let metrics = MergeMetrics::from_counts(&MergeOutcomeCounts {
            total: 50,
            conflicted: 0,
            rolled_back: 0,
            validation_passed: 50,
        });
        assert_eq!(metrics.conflict_rate, 0.0);
        assert!(
            !metrics.auto_merge_ready(),
            "a clean but too-small window must not be ready",
        );
    }

    #[test]
    fn each_threshold_gates_independently() {
        let base = MergeOutcomeCounts {
            total: 200,
            conflicted: 8,          // 4% < 5%
            rolled_back: 1,         // 0.5% < 1%
            validation_passed: 196, // 98% >= 95%
        };
        assert!(MergeMetrics::from_counts(&base).auto_merge_ready());

        // Conflict rate too high (>=5%).
        let conflicty = MergeOutcomeCounts {
            conflicted: 10,
            ..base
        };
        assert!(!MergeMetrics::from_counts(&conflicty).auto_merge_ready());

        // Rollback rate too high (>=1%).
        let rollbacky = MergeOutcomeCounts {
            rolled_back: 2,
            ..base
        };
        assert!(!MergeMetrics::from_counts(&rollbacky).auto_merge_ready());

        // Validation pass rate too low (<95%).
        let flaky = MergeOutcomeCounts {
            validation_passed: 180, // 90%
            ..base
        };
        assert!(!MergeMetrics::from_counts(&flaky).auto_merge_ready());
    }

    #[test]
    fn thresholds_are_strict_or_inclusive_as_documented() {
        // Exactly at the conflict ceiling (5%) is NOT ready (strict `<`).
        let at_conflict_ceiling = MergeMetrics::from_counts(&MergeOutcomeCounts {
            total: 200,
            conflicted: 10, // exactly 5%
            rolled_back: 0,
            validation_passed: 200,
        });
        assert!(!at_conflict_ceiling.auto_merge_ready());

        // Exactly at the validation floor (95%) IS ready (inclusive `>=`),
        // with conflict / rollback clear.
        let at_validation_floor = MergeMetrics::from_counts(&MergeOutcomeCounts {
            total: 200,
            conflicted: 0,
            rolled_back: 0,
            validation_passed: 190, // exactly 95%
        });
        assert!(at_validation_floor.auto_merge_ready());
    }
}
