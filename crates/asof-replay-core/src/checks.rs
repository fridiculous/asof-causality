use crate::{
    Event, EventKey, EventKind, PredictionRecord, ReplayEngine, ReplayError, ReplayOptions,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub results: Vec<CheckResult>,
}

impl CheckReport {
    pub fn passed(&self) -> bool {
        self.results.iter().all(|result| result.passed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckOptions {
    pub max_cutoffs: Option<usize>,
}

impl CheckOptions {
    pub const fn exhaustive() -> Self {
        Self { max_cutoffs: None }
    }

    pub const fn sampled(max_cutoffs: usize) -> Self {
        Self {
            max_cutoffs: Some(if max_cutoffs == 0 { 1 } else { max_cutoffs }),
        }
    }
}

pub fn run_adversarial_checks(events: &[Event]) -> CheckReport {
    run_adversarial_checks_with_options(events, CheckOptions::exhaustive())
}

pub fn run_adversarial_checks_with_options(events: &[Event], options: CheckOptions) -> CheckReport {
    CheckReport {
        results: vec![
            prefix_equivalence(events, options),
            future_mutation(events, options),
            late_arrival(events),
            on_time_vs_late_contrast(events),
            correction_append_only(events),
            label_separation(events),
            deterministic_replay(events),
            audit_invariant(events),
        ],
    }
}

pub fn run_universal_leakage_checks(events: &[Event]) -> CheckReport {
    run_universal_leakage_checks_with_options(events, CheckOptions::exhaustive())
}

pub fn run_universal_leakage_checks_with_options(
    events: &[Event],
    options: CheckOptions,
) -> CheckReport {
    CheckReport {
        results: vec![
            prefix_equivalence(events, options),
            future_mutation(events, options),
            late_arrival(events),
            correction_append_only(events),
            label_separation(events),
            deterministic_replay(events),
            audit_invariant(events),
        ],
    }
}

fn full_predictions(
    events: &[Event],
    compute_labels: bool,
) -> Result<Vec<PredictionRecord>, ReplayError> {
    ReplayEngine::new()
        .replay(events, ReplayOptions { compute_labels })
        .map(|output| output.predictions.records().to_vec())
}

fn predictions_at_or_before(
    predictions: &[PredictionRecord],
    cutoff: (u64, u64),
) -> Vec<PredictionRecord> {
    predictions
        .iter()
        .filter(|record| record.prediction_replay_key() <= cutoff)
        .cloned()
        .collect()
}

fn prediction_cutoffs(events: &[Event]) -> Vec<(u64, u64)> {
    let mut cutoffs: Vec<(u64, u64)> = events
        .iter()
        .filter(|event| event.kind == EventKind::Predict)
        .map(event_replay_key)
        .collect();
    cutoffs.sort_unstable();
    cutoffs.dedup();
    cutoffs
}

fn selected_prediction_cutoffs(
    events: &[Event],
    options: CheckOptions,
) -> (Vec<(u64, u64)>, usize) {
    let cutoffs = prediction_cutoffs(events);
    let total = cutoffs.len();
    let Some(max_cutoffs) = options.max_cutoffs else {
        return (cutoffs, total);
    };

    if cutoffs.len() <= max_cutoffs {
        return (cutoffs, total);
    }

    if max_cutoffs == 1 {
        return (vec![cutoffs[total - 1]], total);
    }

    let mut sampled = Vec::with_capacity(max_cutoffs);
    let last = total - 1;
    for index in 0..max_cutoffs {
        let cutoff_index = index * last / (max_cutoffs - 1);
        let cutoff = cutoffs[cutoff_index];
        if sampled.last().copied() != Some(cutoff) {
            sampled.push(cutoff);
        }
    }

    (sampled, total)
}

fn cutoff_detail(all: &'static str, sampled: &'static str, used: usize, total: usize) -> String {
    if used == total {
        all.to_string()
    } else {
        format!("{sampled} ({used}/{total} deterministic replay-key cutoffs)")
    }
}

fn prefix_equivalence(events: &[Event], options: CheckOptions) -> CheckResult {
    let full = match full_predictions(events, true) {
        Ok(predictions) => predictions,
        Err(error) => return replay_failure("prefix_equivalence", error),
    };
    let (cutoffs, total_cutoffs) = selected_prediction_cutoffs(events, options);

    for cutoff in &cutoffs {
        let prefix_events: Vec<Event> = events
            .iter()
            .filter(|event| event_replay_key(event) <= *cutoff)
            .cloned()
            .collect();
        let prefix = match full_predictions(&prefix_events, true) {
            Ok(predictions) => predictions,
            Err(error) => return replay_failure("prefix_equivalence", error),
        };

        if predictions_at_or_before(&full, *cutoff) != predictions_at_or_before(&prefix, *cutoff) {
            return fail(
                "prefix_equivalence",
                format!("predictions changed for replay-key prefix {cutoff:?}"),
            );
        }
    }

    pass(
        "prefix_equivalence",
        cutoff_detail(
            "all replay-key prefixes matched full replay",
            "sampled replay-key prefixes matched full replay",
            cutoffs.len(),
            total_cutoffs,
        ),
    )
}

fn future_mutation(events: &[Event], options: CheckOptions) -> CheckResult {
    let full = match full_predictions(events, true) {
        Ok(predictions) => predictions,
        Err(error) => return replay_failure("future_mutation", error),
    };
    let (cutoffs, total_cutoffs) = selected_prediction_cutoffs(events, options);

    for cutoff in &cutoffs {
        let mutated: Vec<Event> = events
            .iter()
            .map(|event| {
                if event_replay_key(event) > *cutoff {
                    event.with_mutated_future_payload()
                } else {
                    event.clone()
                }
            })
            .collect();
        let mutated_predictions = match full_predictions(&mutated, true) {
            Ok(predictions) => predictions,
            Err(error) => return replay_failure("future_mutation", error),
        };

        if predictions_at_or_before(&full, *cutoff)
            != predictions_at_or_before(&mutated_predictions, *cutoff)
        {
            return fail(
                "future_mutation",
                format!("future mutation changed predictions at or before {cutoff:?}"),
            );
        }
    }

    pass(
        "future_mutation",
        cutoff_detail(
            "mutating future rows did not change past predictions",
            "mutating sampled future rows did not change past predictions",
            cutoffs.len(),
            total_cutoffs,
        ),
    )
}

fn late_arrival(events: &[Event]) -> CheckResult {
    let predictions = match full_predictions(events, true) {
        Ok(predictions) => predictions,
        Err(error) => return replay_failure("late_arrival", error),
    };
    let update_by_key = update_event_by_key(events);

    for prediction in &predictions {
        let Some(input_key) = prediction.input_event_ids_used.single_key() else {
            continue;
        };
        let Some(event) = update_by_key.get(&input_key) else {
            continue;
        };

        if event.observed_time <= prediction.prediction_time
            && prediction.prediction_replay_key() < event_replay_key(event)
        {
            return fail(
                "late_arrival",
                format!(
                    "prediction at ({}, {}) used late event {} received at ({}, {})",
                    prediction.prediction_time,
                    prediction.prediction_sequence,
                    event.event_id,
                    event.received_time,
                    event.sequence
                ),
            );
        }
    }

    pass(
        "late_arrival",
        "late events were not used before received_time",
    )
}

fn on_time_vs_late_contrast(events: &[Event]) -> CheckResult {
    let baseline = match full_predictions(events, true) {
        Ok(predictions) => predictions,
        Err(error) => return replay_failure("on_time_vs_late_contrast", error),
    };

    for late_event in events
        .iter()
        .filter(|event| event.kind.updates_prediction_state())
    {
        let Some(target_prediction) = baseline.iter().find(|prediction| {
            prediction.symbol == late_event.symbol
                && late_event.observed_time <= prediction.prediction_time
                && prediction.prediction_replay_key() < event_replay_key(late_event)
        }) else {
            continue;
        };

        let mut on_time = events.to_vec();
        for event in &mut on_time {
            if event.event_id == late_event.event_id {
                event.received_time = late_event.observed_time;
                event.sequence = late_event.sequence.saturating_sub(1);
            }
        }

        let on_time_predictions = match full_predictions(&on_time, true) {
            Ok(predictions) => predictions,
            Err(error) => return replay_failure("on_time_vs_late_contrast", error),
        };
        let Some(on_time_record) = on_time_predictions.iter().find(|prediction| {
            prediction.symbol == target_prediction.symbol
                && prediction.prediction_time == target_prediction.prediction_time
                && prediction.prediction_sequence == target_prediction.prediction_sequence
        }) else {
            return fail(
                "on_time_vs_late_contrast",
                "could not find matching on-time prediction",
            );
        };

        if on_time_record.signal_value != target_prediction.signal_value {
            return pass(
                "on_time_vs_late_contrast",
                format!(
                    "moving {} earlier changed prediction at {} from {} to {}",
                    late_event.event_id,
                    target_prediction.prediction_time,
                    target_prediction.signal_value,
                    on_time_record.signal_value
                ),
            );
        }
    }

    fail(
        "on_time_vs_late_contrast",
        "fixture did not contain a late event that changes an in-between prediction",
    )
}

fn correction_append_only(events: &[Event]) -> CheckResult {
    let predictions = match full_predictions(events, true) {
        Ok(predictions) => predictions,
        Err(error) => return replay_failure("correction_append_only", error),
    };
    let update_by_key = update_event_by_key(events);

    for prediction in &predictions {
        let Some(input_key) = prediction.input_event_ids_used.single_key() else {
            continue;
        };
        let Some(correction) = update_by_key.get(&input_key) else {
            continue;
        };

        if correction.kind == EventKind::Correction
            && prediction.prediction_replay_key() < event_replay_key(correction)
        {
            return fail(
                "correction_append_only",
                format!(
                    "prediction at {} used future correction {}",
                    prediction.prediction_time, correction.event_id
                ),
            );
        }
    }

    pass(
        "correction_append_only",
        "corrections did not rewrite predictions emitted before receipt",
    )
}

fn label_separation(events: &[Event]) -> CheckResult {
    let with_labels = match full_predictions(events, true) {
        Ok(predictions) => predictions,
        Err(error) => return replay_failure("label_separation", error),
    };
    let without_labels = match full_predictions(events, false) {
        Ok(predictions) => predictions,
        Err(error) => return replay_failure("label_separation", error),
    };

    if with_labels == without_labels {
        pass(
            "label_separation",
            "disabling labels did not change predictions",
        )
    } else {
        fail(
            "label_separation",
            "label computation changed emitted predictions",
        )
    }
}

fn deterministic_replay(events: &[Event]) -> CheckResult {
    let original = match ReplayEngine::new().replay(events, ReplayOptions::default()) {
        Ok(output) => output.predictions.transcript_hash(),
        Err(error) => return replay_failure("deterministic_replay", error),
    };

    let mut shuffled = events.to_vec();
    shuffled.reverse();
    let shuffled_hash = match ReplayEngine::new().replay(&shuffled, ReplayOptions::default()) {
        Ok(output) => output.predictions.transcript_hash(),
        Err(error) => return replay_failure("deterministic_replay", error),
    };

    if original == shuffled_hash {
        pass(
            "deterministic_replay",
            format!("shuffled input produced transcript hash {original:016x}"),
        )
    } else {
        fail(
            "deterministic_replay",
            format!("hash changed from {original:016x} to {shuffled_hash:016x}"),
        )
    }
}

fn audit_invariant(events: &[Event]) -> CheckResult {
    let predictions = match full_predictions(events, true) {
        Ok(predictions) => predictions,
        Err(error) => return replay_failure("audit_invariant", error),
    };

    for prediction in predictions {
        if prediction.uses_future_input() {
            return fail(
                "audit_invariant",
                format!(
                    "prediction at ({}, {}) used max input at ({}, {})",
                    prediction.prediction_time,
                    prediction.prediction_sequence,
                    prediction.max_input_received_time,
                    prediction.max_input_sequence
                ),
            );
        }
    }

    pass(
        "audit_invariant",
        "all predictions satisfy max_input_replay_key <= prediction_replay_key",
    )
}

fn event_replay_key(event: &Event) -> (u64, u64) {
    (event.received_time, event.sequence)
}

fn update_event_by_key(events: &[Event]) -> BTreeMap<EventKey, &Event> {
    events
        .iter()
        .filter(|event| event.kind.updates_prediction_state())
        .map(|event| (event.event_key, event))
        .collect()
}

fn pass(name: &'static str, detail: impl Into<String>) -> CheckResult {
    CheckResult {
        name,
        passed: true,
        detail: detail.into(),
    }
}

fn fail(name: &'static str, detail: impl Into<String>) -> CheckResult {
    CheckResult {
        name,
        passed: false,
        detail: detail.into(),
    }
}

fn replay_failure(name: &'static str, error: ReplayError) -> CheckResult {
    fail(name, format!("replay failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_pipe_events;

    const FIXTURE: &str = "\
p1|580|580|3|predict|AAPL|
n1|572|585|2|news|AAPL|sentiment=positive
p2|590|590|4|predict|AAPL|
p3|610|610|5|predict|AAPL|
c1|600|615|6|correction|AAPL|sentiment=negative,corrects=n1
p4|620|620|7|predict|AAPL|
l1|640|640|8|label|AAPL|return_bps=12
";

    #[test]
    fn adversarial_checks_pass_for_fixture() {
        let events = parse_pipe_events(FIXTURE).unwrap();
        let report = run_adversarial_checks(&events);
        assert!(report.passed(), "{report:?}");
    }

    #[test]
    fn adversarial_checks_report_replay_errors_without_panicking() {
        let events = parse_pipe_events(
            "\
n1|1|1|1|news|AAPL|
p1|2|2|2|predict|AAPL|
",
        )
        .unwrap();

        let report = run_adversarial_checks(&events);

        assert!(!report.passed());
        assert!(report.results.iter().any(|result| {
            !result.passed
                && result.name == "prefix_equivalence"
                && result.detail.contains("replay failed")
        }));
    }
}
