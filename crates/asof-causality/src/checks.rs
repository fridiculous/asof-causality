use crate::{Event, EventKey, EventRole, PredictionRecord, ReplayEngine, ReplayOptions, Signal};
use std::collections::{BTreeMap, BTreeSet};

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
                format!("PredictionRecords changed for received-time prefix {cutoff}"),
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
                format!("future mutation changed PredictionRecords at or before {cutoff}"),
            );
        }
    }

    pass(
        "future_mutation",
        cutoff_detail(
            "mutating future rows did not change prior PredictionRecords",
            "mutating sampled future rows did not change prior PredictionRecords",
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
        let moved_received_time = late_event.observed_time;
        let before_sequence = (moved_received_time == target_prediction.prediction_time)
            .then_some(target_prediction.prediction_received_sequence_number);
        let Some(moved_received_sequence_number) = available_received_sequence_number(
            &on_time,
            late_event.event_id.as_str(),
            moved_received_time,
            before_sequence,
        ) else {
            continue;
        };
        for event in &mut on_time {
            if event.event_id == late_event.event_id {
                event.received_time = moved_received_time;
                event.received_sequence_number = moved_received_sequence_number;
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
                    "moving {} earlier changed SignalEvaluation at {} from {} to {}",
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
        "fixture did not contain a late event that changes an in-between SignalEvaluation",
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
                        "PredictionRecord at {} used future feature correction {}",
                        prediction.prediction_time, feature_correction.event_id
                    ),
                );
            }
        }
    }

    pass(
        "feature_correction_append_only",
        "feature corrections did not rewrite prior PredictionRecords",
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
            "disabling outcomes did not change PredictionRecords",
        )
    } else {
        fail(
            "outcome_separation",
            "outcome computation changed PredictionRecords",
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
        "all PredictionRecords satisfy max_input_replay_key <= prediction_replay_key",
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

fn available_received_sequence_number(
    events: &[Event],
    moving_event_id: &str,
    received_time: u64,
    before_sequence: Option<u64>,
) -> Option<u64> {
    let used = events
        .iter()
        .filter(|event| event.event_id != moving_event_id && event.received_time == received_time)
        .map(|event| event.received_sequence_number)
        .collect::<BTreeSet<_>>();

    let mut candidate = 0_u64;
    loop {
        if before_sequence.is_some_and(|limit| candidate >= limit) {
            return None;
        }
        if !used.contains(&candidate) {
            return Some(candidate);
        }
        candidate = candidate.checked_add(1)?;
    }
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
        parse_pipe_events, AsOfView, InputSet, SignalEvaluation, SymbolSlot, FIXED_DECIMAL_SCALE,
    };
    use proptest::prelude::*;
    use proptest::test_runner::TestCaseError;

    const FIXTURE: &str = "\
p1|580|580|3|prediction|AAPL|
n1|572|585|2|feature|AAPL|sentiment=positive
p2|590|590|4|prediction|AAPL|
p3|610|610|5|prediction|AAPL|
c1|600|615|6|feature_correction|AAPL|sentiment=negative,corrects=n1
p4|620|620|7|prediction|AAPL|
l1|640|640|8|outcome|AAPL|return_bps=12
";

    fn arb_event_stream() -> impl Strategy<Value = Vec<Event>> {
        prop::collection::vec(
            (
                0_u64..24,
                0_u64..4,
                0_u64..8,
                0_u8..5,
                0_u8..5,
                -20_i32..=20,
                any::<u64>(),
            ),
            0..24,
        )
        .prop_map(|specs| {
            let mut random_events = specs
                .into_iter()
                .enumerate()
                .map(
                    |(
                        index,
                        (
                            observed_bucket,
                            lag_bucket,
                            sequence_bucket,
                            role_code,
                            symbol_index,
                            payload_value,
                            shuffle_key,
                        ),
                    )| {
                        let role = match role_code {
                            0 | 1 => EventRole::Feature,
                            2 => EventRole::FeatureCorrection,
                            3 => EventRole::Prediction,
                            _ => EventRole::Outcome,
                        };
                        let observed_time = 2_000 + observed_bucket * 10;
                        let received_time = observed_time + lag_bucket * 10;
                        let received_sequence_number = 20 + index as u64;
                        let symbol = format!("SYM{symbol_index}");
                        let event_id = format!("r{index}_{observed_bucket}_{sequence_bucket}");
                        let payload = random_payload(role, payload_value, index);

                        (
                            shuffle_key,
                            index,
                            Event::new(
                                event_id,
                                observed_time,
                                received_time,
                                received_sequence_number,
                                role,
                                symbol,
                                payload,
                            ),
                        )
                    },
                )
                .collect::<Vec<_>>();
            random_events.sort_by_key(|(shuffle_key, index, _)| (*shuffle_key, *index));

            let mut events = sentinel_events();
            events.extend(random_events.into_iter().map(|(_, _, event)| event));
            events
        })
    }

    fn random_payload(role: EventRole, payload_value: i32, index: usize) -> String {
        match role {
            EventRole::Feature | EventRole::FeatureCorrection => {
                if index.is_multiple_of(2) {
                    format!("score={payload_value}")
                } else {
                    let sentiment = match payload_value.rem_euclid(3) {
                        0 => "negative",
                        1 => "neutral",
                        _ => "positive",
                    };
                    format!("sentiment={sentiment},corrects=r{index}")
                }
            }
            EventRole::Prediction => String::new(),
            EventRole::Outcome => format!("return_bps={payload_value}"),
        }
    }

    fn sentinel_events() -> Vec<Event> {
        vec![
            Event::new(
                "p_prop_before",
                1_000,
                1_000,
                1,
                EventRole::Prediction,
                "PROP_SENT",
                "",
            ),
            Event::new(
                "n_prop_late",
                1_000,
                1_020,
                2,
                EventRole::Feature,
                "PROP_SENT",
                "sentiment=positive",
            ),
            Event::new(
                "p_prop_between",
                1_010,
                1_010,
                3,
                EventRole::Prediction,
                "PROP_SENT",
                "",
            ),
            Event::new(
                "p_prop_after",
                1_030,
                1_030,
                4,
                EventRole::Prediction,
                "PROP_SENT",
                "",
            ),
            Event::new(
                "c_prop_late",
                1_040,
                1_060,
                5,
                EventRole::FeatureCorrection,
                "PROP_SENT",
                "sentiment=negative,corrects=n_prop_late",
            ),
            Event::new(
                "p_prop_before_correction",
                1_050,
                1_050,
                6,
                EventRole::Prediction,
                "PROP_SENT",
                "",
            ),
            Event::new(
                "p_prop_after_correction",
                1_070,
                1_070,
                7,
                EventRole::Prediction,
                "PROP_SENT",
                "",
            ),
            Event::new(
                "score_seed_1",
                1_200,
                1_200,
                8,
                EventRole::Feature,
                "SCORE_SENT",
                "score=10",
            ),
            Event::new(
                "score_seed_2",
                1_210,
                1_210,
                9,
                EventRole::Feature,
                "SCORE_SENT",
                "score=10",
            ),
            Event::new(
                "score_seed_3",
                1_220,
                1_220,
                10,
                EventRole::Feature,
                "SCORE_SENT",
                "score=10",
            ),
            Event::new(
                "score_late_spike",
                1_230,
                1_250,
                11,
                EventRole::Feature,
                "SCORE_SENT",
                "score=30",
            ),
            Event::new(
                "p_score_between",
                1_240,
                1_240,
                12,
                EventRole::Prediction,
                "SCORE_SENT",
                "",
            ),
            Event::new(
                "p_score_after",
                1_260,
                1_260,
                13,
                EventRole::Prediction,
                "SCORE_SENT",
                "",
            ),
        ]
    }

    fn assert_check_result(result: CheckResult) -> Result<(), TestCaseError> {
        prop_assert!(result.passed, "{} failed: {}", result.name, result.detail);
        Ok(())
    }

    #[derive(Clone, Copy)]
    struct LastFeatureTestSignal;

    impl Signal for LastFeatureTestSignal {
        fn name(&self) -> &'static str {
            "last-feature-sentiment"
        }

        fn evaluate(
            &self,
            view: AsOfView<'_>,
            symbol: SymbolSlot,
            _as_of_timestamp: u64,
        ) -> SignalEvaluation {
            view.snapshot(symbol)
        }
    }

    #[derive(Clone, Copy)]
    struct WindowedFeatureTestSignal {
        window: usize,
    }

    impl Signal for WindowedFeatureTestSignal {
        fn name(&self) -> &'static str {
            "windowed-feature-sentiment"
        }

        fn config_descriptor(&self) -> String {
            format!("window={}", self.window)
        }

        fn evaluate(
            &self,
            view: AsOfView<'_>,
            symbol: SymbolSlot,
            _as_of_timestamp: u64,
        ) -> SignalEvaluation {
            view.windowed_snapshot(symbol, self.window)
        }
    }

    #[derive(Clone, Copy)]
    struct ZScoreTestSignal;

    impl Signal for ZScoreTestSignal {
        fn name(&self) -> &'static str {
            "windowed-zscore"
        }

        fn config_descriptor(&self) -> String {
            "window=5;threshold=1".to_string()
        }

        fn evaluate(
            &self,
            view: AsOfView<'_>,
            symbol: SymbolSlot,
            _as_of_timestamp: u64,
        ) -> SignalEvaluation {
            view.score_window_snapshot(symbol, 5, FIXED_DECIMAL_SCALE)
        }
    }

    fn assert_default_transcript_hash_stable(events: &[Event]) -> Result<(), TestCaseError> {
        let original = ReplayEngine::with_signal(LastFeatureTestSignal)
            .replay(events, ReplayOptions::default())
            .map_err(|error| TestCaseError::fail(format!("original replay failed: {error}")))?
            .predictions
            .transcript_hash();

        for shuffled in [
            reversed(events),
            deterministic_permutation(events, 0x1234_5678_9abc_def0),
            deterministic_permutation(events, 0x0ddc_0ffe_e15e_f00d),
        ] {
            let shuffled_hash = ReplayEngine::with_signal(LastFeatureTestSignal)
                .replay(&shuffled, ReplayOptions::default())
                .map_err(|error| TestCaseError::fail(format!("shuffled replay failed: {error}")))?
                .predictions
                .transcript_hash();
            prop_assert_eq!(original, shuffled_hash);
        }

        Ok(())
    }

    fn reversed(events: &[Event]) -> Vec<Event> {
        let mut reversed = events.to_vec();
        reversed.reverse();
        reversed
    }

    fn deterministic_permutation(events: &[Event], seed: u64) -> Vec<Event> {
        let mut shuffled = events.to_vec();
        let mut state = seed;
        for index in (1..shuffled.len()).rev() {
            state = splitmix64_next(state);
            let swap_index = (state as usize) % (index + 1);
            shuffled.swap(index, swap_index);
        }
        shuffled
    }

    fn splitmix64_next(mut state: u64) -> u64 {
        state = state.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn generated_streams_pass_all_adversarial_checks(events in arb_event_stream()) {
            let report = run_adversarial_checks_with_options_for_signal(&events, CheckOptions::exhaustive(), LastFeatureTestSignal);
            prop_assert!(report.passed(), "{report:?}");
        }

        #[test]
        fn generated_streams_pass_windowed_zscore_checks(events in arb_event_stream()) {
            let report = run_adversarial_checks_with_options_for_signal(
                &events,
                CheckOptions::exhaustive(),
                ZScoreTestSignal,
            );
            prop_assert!(report.passed(), "{report:?}");
        }

        #[test]
        fn generated_streams_preserve_prefix_equivalence(events in arb_event_stream()) {
            assert_check_result(prefix_equivalence(
                &events,
                CheckOptions::exhaustive(),
                &LastFeatureTestSignal,
            ))?;
        }

        #[test]
        fn generated_streams_ignore_future_payload_mutations(events in arb_event_stream()) {
            assert_check_result(future_mutation(
                &events,
                CheckOptions::exhaustive(),
                &LastFeatureTestSignal,
            ))?;
        }

        #[test]
        fn generated_streams_never_use_late_inputs_early(events in arb_event_stream()) {
            assert_check_result(late_arrival(&events, &LastFeatureTestSignal))?;
        }

        #[test]
        fn generated_streams_have_non_vacuous_late_contrast(events in arb_event_stream()) {
            assert_check_result(on_time_vs_late_contrast(
                &events,
                &LastFeatureTestSignal,
            ))?;
        }

        #[test]
        fn generated_streams_keep_feature_corrections_append_only(events in arb_event_stream()) {
            assert_check_result(feature_correction_append_only(
                &events,
                &LastFeatureTestSignal,
            ))?;
        }

        #[test]
        fn generated_streams_keep_outcomes_separate(events in arb_event_stream()) {
            assert_check_result(outcome_separation(&events, &LastFeatureTestSignal))?;
        }

        #[test]
        fn generated_streams_replay_deterministically(events in arb_event_stream()) {
            assert_check_result(deterministic_replay(&events, &LastFeatureTestSignal))?;
            assert_default_transcript_hash_stable(&events)?;
        }

        #[test]
        fn generated_streams_satisfy_audit_invariant(events in arb_event_stream()) {
            assert_check_result(audit_invariant(&events, &LastFeatureTestSignal))?;
        }
    }

    #[test]
    fn adversarial_checks_pass_for_fixture() {
        let events = parse_pipe_events(FIXTURE).unwrap();
        let report = run_adversarial_checks_with_options_for_signal(
            &events,
            CheckOptions::exhaustive(),
            LastFeatureTestSignal,
        );
        assert!(report.passed(), "{report:?}");
    }

    #[test]
    fn adversarial_checks_pass_for_windowed_signal() {
        let events = parse_pipe_events(FIXTURE).unwrap();
        let report = run_adversarial_checks_with_options_for_signal(
            &events,
            CheckOptions::exhaustive(),
            WindowedFeatureTestSignal { window: 5 },
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

        let report = run_adversarial_checks_with_options_for_signal(
            &events,
            CheckOptions::exhaustive(),
            LastFeatureTestSignal,
        );

        assert!(!report.passed());
        assert!(report
            .results
            .iter()
            .any(|result| !result.passed && result.detail.contains("replay failed")));
    }

    #[test]
    fn available_received_sequence_number_returns_none_when_prefix_is_full() {
        let events = parse_pipe_events(
            "\
f0|100|100|0|feature|XYZ|sentiment=positive
f1|100|100|1|feature|XYZ|sentiment=negative
moving|90|110|2|feature|XYZ|sentiment=positive
",
        )
        .unwrap();

        assert_eq!(
            available_received_sequence_number(&events, "moving", 100, Some(2)),
            None
        );
    }

    #[test]
    fn available_received_sequence_number_finds_unused_prefix_slot() {
        let events = parse_pipe_events(
            "\
f0|100|100|0|feature|XYZ|sentiment=positive
f2|100|100|2|feature|XYZ|sentiment=negative
moving|90|110|3|feature|XYZ|sentiment=positive
",
        )
        .unwrap();

        assert_eq!(
            available_received_sequence_number(&events, "moving", 100, Some(3)),
            Some(1)
        );
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

            fn evaluate(
                &self,
                _view: AsOfView<'_>,
                _symbol: crate::SymbolSlot,
                _as_of_timestamp: u64,
            ) -> SignalEvaluation {
                SignalEvaluation {
                    signal_value: 1,
                    input_event_ids_used: InputSet::one(self.input_key),
                    max_input_received_time: 100,
                    max_input_received_sequence_number: 2,
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
