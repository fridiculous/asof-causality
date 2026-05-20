use crate::{
    Event, EventKey, EventRole, LastFeatureSentimentSignal, PredictionRecord, ReplayEngine,
    ReplayOptions, Signal,
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
        let max_cutoffs = if max_cutoffs == 0 { 1 } else { max_cutoffs };
        Self {
            max_cutoffs: Some(max_cutoffs),
        }
    }
}

pub fn run_adversarial_checks(events: &[Event]) -> CheckReport {
    run_adversarial_checks_with_options(events, CheckOptions::exhaustive())
}

pub fn run_adversarial_checks_with_options(events: &[Event], options: CheckOptions) -> CheckReport {
    run_adversarial_checks_with_options_for_signal(events, options, LastFeatureSentimentSignal)
}

pub fn run_adversarial_checks_with_options_for_signal<S>(
    events: &[Event],
    options: CheckOptions,
    signal: S,
) -> CheckReport
where
    S: Signal + Clone,
{
    CheckReport {
        results: vec![
            prefix_equivalence(events, options, &signal),
            future_mutation(events, options, &signal),
            late_arrival(events, &signal),
            on_time_vs_late_contrast(events, &signal),
            feature_correction_append_only(events, &signal),
            outcome_separation(events, &signal),
            deterministic_replay(events, &signal),
            audit_invariant(events, &signal),
        ],
    }
}

fn full_predictions<S>(
    check_name: &'static str,
    events: &[Event],
    compute_outcomes: bool,
    signal: &S,
) -> Result<Vec<PredictionRecord>, CheckResult>
where
    S: Signal + Clone,
{
    ReplayEngine::with_signal(signal.clone())
        .replay(events, ReplayOptions { compute_outcomes })
        .map(|output| output.predictions.records().to_vec())
        .map_err(|error| fail(check_name, format!("replay failed: {error}")))
}

fn predictions_at_or_before(
    predictions: &[PredictionRecord],
    cutoff: u64,
) -> Vec<PredictionRecord> {
    predictions
        .iter()
        .filter(|record| record.prediction_time <= cutoff)
        .cloned()
        .collect()
}

fn prediction_cutoffs(events: &[Event]) -> Vec<u64> {
    let mut cutoffs: Vec<u64> = events
        .iter()
        .filter(|event| event.role == EventRole::Prediction)
        .map(|event| event.received_time)
        .collect();
    cutoffs.sort_unstable();
    cutoffs.dedup();
    cutoffs
}

fn selected_prediction_cutoffs(events: &[Event], options: CheckOptions) -> (Vec<u64>, usize) {
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
        format!("{sampled} ({used}/{total} deterministic received-time cutoffs)")
    }
}

fn prefix_equivalence<S>(events: &[Event], options: CheckOptions, signal: &S) -> CheckResult
where
    S: Signal + Clone,
{
    let full = match full_predictions("prefix_equivalence", events, true, signal) {
        Ok(predictions) => predictions,
        Err(result) => return result,
    };
    let (cutoffs, total_cutoffs) = selected_prediction_cutoffs(events, options);

    for cutoff in &cutoffs {
        let prefix_events: Vec<Event> = events
            .iter()
            .filter(|event| event.received_time <= *cutoff)
            .cloned()
            .collect();
        let prefix = match full_predictions("prefix_equivalence", &prefix_events, true, signal) {
            Ok(predictions) => predictions,
            Err(result) => return result,
        };

        if predictions_at_or_before(&full, *cutoff) != predictions_at_or_before(&prefix, *cutoff) {
            return fail(
                "prefix_equivalence",
                format!("predictions changed for received-time prefix {cutoff}"),
            );
        }
    }

    pass(
        "prefix_equivalence",
        cutoff_detail(
            "all received-time prefixes matched full replay",
            "sampled received-time prefixes matched full replay",
            cutoffs.len(),
            total_cutoffs,
        ),
    )
}

fn future_mutation<S>(events: &[Event], options: CheckOptions, signal: &S) -> CheckResult
where
    S: Signal + Clone,
{
    let full = match full_predictions("future_mutation", events, true, signal) {
        Ok(predictions) => predictions,
        Err(result) => return result,
    };
    let (cutoffs, total_cutoffs) = selected_prediction_cutoffs(events, options);

    for cutoff in &cutoffs {
        let mutated: Vec<Event> = events
            .iter()
            .map(|event| {
                if event.received_time > *cutoff {
                    event.with_mutated_future_payload()
                } else {
                    event.clone()
                }
            })
            .collect();
        let mutated_predictions = match full_predictions("future_mutation", &mutated, true, signal)
        {
            Ok(predictions) => predictions,
            Err(result) => return result,
        };

        if predictions_at_or_before(&full, *cutoff)
            != predictions_at_or_before(&mutated_predictions, *cutoff)
        {
            return fail(
                "future_mutation",
                format!("future mutation changed predictions at or before {cutoff}"),
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

fn late_arrival<S>(events: &[Event], signal: &S) -> CheckResult
where
    S: Signal + Clone,
{
    let predictions = match full_predictions("late_arrival", events, true, signal) {
        Ok(predictions) => predictions,
        Err(result) => return result,
    };
    let event_by_key = event_by_key(events);

    for prediction in &predictions {
        let Some(prediction_event) = event_by_key.get(&prediction.prediction_event_key) else {
            return fail(
                "late_arrival",
                format!(
                    "missing prediction event for key {}",
                    prediction.prediction_event_key.0
                ),
            );
        };

        for input_key in prediction.input_event_ids_used.iter() {
            let Some(event) = event_by_key.get(&input_key) else {
                continue;
            };

            if event.observed_time <= prediction.prediction_time
                && event.replay_key() > prediction_event.replay_key()
            {
                return fail(
                    "late_arrival",
                    format!(
                        "prediction {} at replay key {:?} used late event {} at replay key {:?}",
                        prediction_event.event_id,
                        prediction_event.replay_key(),
                        event.event_id,
                        event.replay_key()
                    ),
                );
            }
        }
    }

    pass(
        "late_arrival",
        "late events were not used before their replay key",
    )
}

fn on_time_vs_late_contrast<S>(events: &[Event], signal: &S) -> CheckResult
where
    S: Signal + Clone,
{
    let baseline = match full_predictions("on_time_vs_late_contrast", events, true, signal) {
        Ok(predictions) => predictions,
        Err(result) => return result,
    };

    for late_event in events
        .iter()
        .filter(|event| event.role.updates_signal_state())
    {
        let Some(target_prediction) = baseline.iter().find(|prediction| {
            prediction.symbol == late_event.symbol_key
                && late_event.observed_time <= prediction.prediction_time
                && prediction.prediction_time < late_event.received_time
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

        let on_time_predictions =
            match full_predictions("on_time_vs_late_contrast", &on_time, true, signal) {
                Ok(predictions) => predictions,
                Err(result) => return result,
            };
        let Some(on_time_record) = on_time_predictions.iter().find(|prediction| {
            prediction.symbol == target_prediction.symbol
                && prediction.prediction_time == target_prediction.prediction_time
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

fn feature_correction_append_only<S>(events: &[Event], signal: &S) -> CheckResult
where
    S: Signal + Clone,
{
    let predictions = match full_predictions("feature_correction_append_only", events, true, signal)
    {
        Ok(predictions) => predictions,
        Err(result) => return result,
    };
    let update_by_key = update_event_by_key(events);

    for prediction in &predictions {
        for input_key in prediction.input_event_ids_used.iter() {
            let Some(feature_correction) = update_by_key.get(&input_key) else {
                continue;
            };

            if feature_correction.role == EventRole::FeatureCorrection
                && prediction.prediction_time < feature_correction.received_time
            {
                return fail(
                    "feature_correction_append_only",
                    format!(
                        "prediction at {} used future feature correction {}",
                        prediction.prediction_time, feature_correction.event_id
                    ),
                );
            }
        }
    }

    pass(
        "feature_correction_append_only",
        "feature corrections did not rewrite predictions emitted before receipt",
    )
}

fn outcome_separation<S>(events: &[Event], signal: &S) -> CheckResult
where
    S: Signal + Clone,
{
    let with_outcomes = match full_predictions("outcome_separation", events, true, signal) {
        Ok(predictions) => predictions,
        Err(result) => return result,
    };
    let without_outcomes = match full_predictions("outcome_separation", events, false, signal) {
        Ok(predictions) => predictions,
        Err(result) => return result,
    };

    if with_outcomes == without_outcomes {
        pass(
            "outcome_separation",
            "disabling outcomes did not change predictions",
        )
    } else {
        fail(
            "outcome_separation",
            "outcome computation changed emitted predictions",
        )
    }
}

fn deterministic_replay<S>(events: &[Event], signal: &S) -> CheckResult
where
    S: Signal + Clone,
{
    let original =
        match ReplayEngine::with_signal(signal.clone()).replay(events, ReplayOptions::default()) {
            Ok(output) => output.predictions.transcript_hash(),
            Err(error) => {
                return fail("deterministic_replay", format!("replay failed: {error}"));
            }
        };

    let mut shuffled = events.to_vec();
    shuffled.reverse();
    let shuffled_hash = match ReplayEngine::with_signal(signal.clone())
        .replay(&shuffled, ReplayOptions::default())
    {
        Ok(output) => output.predictions.transcript_hash(),
        Err(error) => {
            return fail("deterministic_replay", format!("replay failed: {error}"));
        }
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

fn audit_invariant<S>(events: &[Event], signal: &S) -> CheckResult
where
    S: Signal + Clone,
{
    let predictions = match full_predictions("audit_invariant", events, true, signal) {
        Ok(predictions) => predictions,
        Err(result) => return result,
    };
    let event_by_key = event_by_key(events);

    for prediction in predictions {
        let Some(prediction_event) = event_by_key.get(&prediction.prediction_event_key) else {
            return fail(
                "audit_invariant",
                format!(
                    "missing prediction event for key {}",
                    prediction.prediction_event_key.0
                ),
            );
        };

        for input_key in prediction.input_event_ids_used.iter() {
            let Some(input_event) = event_by_key.get(&input_key) else {
                return fail(
                    "audit_invariant",
                    format!("missing input event for key {}", input_key.0),
                );
            };

            if input_event.replay_key() > prediction_event.replay_key() {
                return fail(
                    "audit_invariant",
                    format!(
                        "prediction {} used future replay key {}",
                        prediction_event.event_id, input_event.event_id
                    ),
                );
            }
        }
    }

    pass(
        "audit_invariant",
        "all predictions satisfy max_input_replay_key <= prediction_replay_key",
    )
}

fn event_by_key(events: &[Event]) -> BTreeMap<EventKey, &Event> {
    events
        .iter()
        .map(|event| (event.event_key, event))
        .collect()
}

fn update_event_by_key(events: &[Event]) -> BTreeMap<EventKey, &Event> {
    events
        .iter()
        .filter(|event| event.role.updates_signal_state())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        parse_pipe_events, AsOfView, InputSet, SymbolSnapshot, WindowedFeatureSentimentSignal,
    };

    const FIXTURE: &str = "\
p1|580|580|3|prediction|AAPL|
n1|572|585|2|feature|AAPL|sentiment=positive
p2|590|590|4|prediction|AAPL|
p3|610|610|5|prediction|AAPL|
c1|600|615|6|feature_correction|AAPL|sentiment=negative,corrects=n1
p4|620|620|7|prediction|AAPL|
l1|640|640|8|outcome|AAPL|return_bps=12
";

    #[test]
    fn adversarial_checks_pass_for_fixture() {
        let events = parse_pipe_events(FIXTURE).unwrap();
        let report = run_adversarial_checks(&events);
        assert!(report.passed(), "{report:?}");
    }

    #[test]
    fn adversarial_checks_pass_for_windowed_signal() {
        let events = parse_pipe_events(FIXTURE).unwrap();
        let report = run_adversarial_checks_with_options_for_signal(
            &events,
            CheckOptions::exhaustive(),
            WindowedFeatureSentimentSignal::new(5),
        );

        assert!(report.passed(), "{report:?}");
    }

    #[test]
    fn replay_errors_become_failed_check_results() {
        let events = parse_pipe_events(
            "\
f1|100|100|1|feature|XYZ|
p1|110|110|2|prediction|XYZ|
",
        )
        .unwrap();

        let report = run_adversarial_checks(&events);

        assert!(!report.passed());
        assert!(report
            .results
            .iter()
            .any(|result| !result.passed && result.detail.contains("replay failed")));
    }

    #[test]
    fn late_arrival_uses_full_replay_key() {
        let events = parse_pipe_events(
            "\
p1|100|100|1|prediction|XYZ|
f1|90|100|2|feature|XYZ|sentiment=positive
",
        )
        .unwrap();
        let future_key = events
            .iter()
            .find(|event| event.event_id == "f1")
            .unwrap()
            .event_key;

        #[derive(Clone, Copy)]
        struct LyingSignal {
            input_key: EventKey,
        }

        impl Signal for LyingSignal {
            fn name(&self) -> &'static str {
                "lying-signal"
            }

            fn predict(
                &self,
                _view: AsOfView<'_>,
                _symbol: crate::SymbolId,
                _prediction_time: u64,
            ) -> SymbolSnapshot {
                SymbolSnapshot {
                    signal_value: 1,
                    input_event_ids_used: InputSet::one(self.input_key),
                    max_input_received_time: 100,
                    max_input_sequence: 2,
                    max_input_event_key: Some(self.input_key),
                    feature_recipe_hash: None,
                }
            }
        }

        let result = late_arrival(
            &events,
            &LyingSignal {
                input_key: future_key,
            },
        );

        assert!(!result.passed);
        assert!(result.detail.contains("used late event f1"));
        assert!(result.detail.contains("replay key"));
    }

    #[test]
    fn sampled_zero_is_not_exhaustive() {
        let events = parse_pipe_events(FIXTURE).unwrap();
        let (cutoffs, total) = selected_prediction_cutoffs(&events, CheckOptions::sampled(0));

        assert!(total > 1);
        assert_eq!(cutoffs.len(), 1);
    }
}
