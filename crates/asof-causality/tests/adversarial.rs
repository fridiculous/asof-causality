use asof_causality::{
    parse_pipe_events, run_adversarial_checks_with_options_for_signal, AsOfView, CheckOptions,
    EventRole, FixedDecimal, ReplayEngine, ReplayOptions, ReplayOrder, Signal, SignalEvaluation,
    SymbolSlot, FIXED_DECIMAL_SCALE,
};

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

#[derive(Clone, Copy)]
struct VolAdjustedMomentumTestSignal;

impl Signal for VolAdjustedMomentumTestSignal {
    fn name(&self) -> &'static str {
        "vol-adjusted-momentum"
    }

    fn config_descriptor(&self) -> String {
        "fast_window=2;slow_window=4;min_trend=0;volatility_divisor=2".to_string()
    }

    fn evaluate(
        &self,
        view: AsOfView<'_>,
        symbol: SymbolSlot,
        _as_of_timestamp: u64,
    ) -> SignalEvaluation {
        view.score_momentum_snapshot(symbol, 2, 4, FixedDecimal::from_scaled(0), 2)
    }
}

fn fixture_events() -> Vec<asof_causality::Event> {
    parse_pipe_events(include_str!("../../../examples/late-arrival.pipe")).unwrap()
}

fn negative_control_events() -> Vec<asof_causality::Event> {
    parse_pipe_events(include_str!(
        "../../../examples/lookahead-negative-control.pipe"
    ))
    .unwrap()
}

fn zscore_events() -> Vec<asof_causality::Event> {
    parse_pipe_events(include_str!("../../../examples/zscore-lookahead.pipe")).unwrap()
}

fn alfred_events() -> Vec<asof_causality::Event> {
    parse_pipe_events(include_str!("../../../examples/alfred-dgs10-sp500.pipe")).unwrap()
}

fn alfred_payems_revision_events() -> Vec<asof_causality::Event> {
    parse_pipe_events(include_str!(
        "../../../examples/alfred-payems-revision.pipe"
    ))
    .unwrap()
}

fn alfred_payems_large_revision_events() -> Vec<asof_causality::Event> {
    parse_pipe_events(include_str!(
        "../../../examples/alfred-payems-revisions-2020.pipe"
    ))
    .unwrap()
}

fn assert_check_passes(name: &str) {
    let events = fixture_events();
    let report = run_adversarial_checks_with_options_for_signal(
        &events,
        CheckOptions::exhaustive(),
        LastFeatureTestSignal,
    );
    let result = report
        .results
        .iter()
        .find(|result| result.name == name)
        .unwrap_or_else(|| panic!("missing check {name}"));

    assert!(result.passed, "{} failed: {}", result.name, result.detail);
}

#[test]
fn prefix_equivalence_holds() {
    assert_check_passes("prefix_equivalence");
}

#[test]
fn future_mutation_does_not_change_past_predictions() {
    assert_check_passes("future_mutation");
}

#[test]
fn late_arrival_is_not_used_before_received_time() {
    assert_check_passes("late_arrival");
}

#[test]
fn on_time_vs_late_arrival_can_change_prediction() {
    assert_check_passes("on_time_vs_late_contrast");
}

#[test]
fn feature_corrections_are_append_only() {
    assert_check_passes("feature_correction_append_only");
}

#[test]
fn outcomes_do_not_affect_predictions() {
    assert_check_passes("outcome_separation");
}

#[test]
fn shuffled_input_replays_to_same_transcript_hash() {
    assert_check_passes("deterministic_replay");
}

#[test]
fn prediction_audit_invariant_holds() {
    assert_check_passes("audit_invariant");
}

#[test]
fn disabling_outcome_computation_does_not_change_transcript_hash() {
    let events = fixture_events();
    let with_outcomes = ReplayEngine::with_signal(LastFeatureTestSignal)
        .replay(
            &events,
            ReplayOptions {
                compute_outcomes: true,
            },
        )
        .unwrap()
        .predictions
        .transcript_hash();
    let without_outcomes = ReplayEngine::with_signal(LastFeatureTestSignal)
        .replay(
            &events,
            ReplayOptions {
                compute_outcomes: false,
            },
        )
        .unwrap()
        .predictions
        .transcript_hash();

    assert_eq!(with_outcomes, without_outcomes);
}

#[test]
fn received_time_engine_survives_negative_control() {
    let events = negative_control_events();
    let output = ReplayEngine::with_signal(LastFeatureTestSignal)
        .replay_with_order(&events, ReplayOptions::default(), ReplayOrder::ReceivedTime)
        .unwrap();

    assert!(output.predictions.impossible_predictions().is_empty());
}

#[test]
fn observed_time_baseline_leaks_on_negative_control() {
    let events = negative_control_events();
    let output = ReplayEngine::with_signal(LastFeatureTestSignal)
        .replay_with_order(
            &events,
            ReplayOptions::default(),
            ReplayOrder::ObservedTimeLeaky,
        )
        .unwrap();

    let same_time_sequence_leak = output
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_time == 95)
        .expect("negative fixture should emit prediction at 95");

    assert_eq!(same_time_sequence_leak.max_input_received_time, 95);
    assert_eq!(
        same_time_sequence_leak.prediction_received_sequence_number,
        4
    );
    assert_eq!(
        same_time_sequence_leak.max_input_received_sequence_number,
        5
    );

    let late_feature_leak = output
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_time == 120)
        .expect("negative fixture should emit prediction at 120");

    assert_eq!(late_feature_leak.max_input_received_time, 150);
    assert!(late_feature_leak.max_input_received_time > late_feature_leak.prediction_time);

    let feature_correction_leak = output
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_time == 170)
        .expect("negative fixture should emit prediction at 170");

    assert_eq!(feature_correction_leak.max_input_received_time, 180);
    assert!(
        feature_correction_leak.max_input_received_time > feature_correction_leak.prediction_time
    );

    assert_eq!(output.predictions.impossible_predictions().len(), 3);
}

#[test]
fn windowed_signal_records_multi_input_provenance() {
    let events = negative_control_events();
    let output = ReplayEngine::with_signal(WindowedFeatureTestSignal { window: 5 })
        .replay_with_order(&events, ReplayOptions::default(), ReplayOrder::ReceivedTime)
        .unwrap();

    let before_late_feature = output
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_time == 120)
        .expect("negative fixture should emit prediction at 120");

    assert_eq!(before_late_feature.input_event_ids_used.len(), 4);
    assert_eq!(before_late_feature.max_input_received_time, 95);
    assert_eq!(before_late_feature.max_input_received_sequence_number, 5);
}

#[test]
fn zscore_fixture_passes_adversarial_checks() {
    let events = zscore_events();
    let report = asof_causality::run_adversarial_checks_with_options_for_signal(
        &events,
        asof_causality::CheckOptions::exhaustive(),
        ZScoreTestSignal,
    );

    assert!(report.passed(), "{report:?}");
}

#[test]
fn observed_time_baseline_leaks_numeric_zscore_input() {
    let events = zscore_events();
    let output = ReplayEngine::with_signal(ZScoreTestSignal)
        .replay_with_order(
            &events,
            ReplayOptions::default(),
            ReplayOrder::ObservedTimeLeaky,
        )
        .unwrap();

    let leaked = output
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_time == 100)
        .expect("zscore fixture should emit prediction at 100");

    assert_eq!(leaked.signal_value, 1);
    assert_eq!(leaked.max_input_received_time, 120);
    assert_eq!(output.predictions.impossible_predictions().len(), 1);
}

#[test]
fn vol_adjusted_momentum_fixture_passes_adversarial_checks() {
    let events = zscore_events();
    let report = asof_causality::run_adversarial_checks_with_options_for_signal(
        &events,
        asof_causality::CheckOptions::exhaustive(),
        VolAdjustedMomentumTestSignal,
    );

    assert!(report.passed(), "{report:?}");
}

#[test]
fn observed_time_baseline_leaks_vol_adjusted_momentum_input() {
    let events = zscore_events();
    let output = ReplayEngine::with_signal(VolAdjustedMomentumTestSignal)
        .replay_with_order(
            &events,
            ReplayOptions::default(),
            ReplayOrder::ObservedTimeLeaky,
        )
        .unwrap();

    let leaked = output
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_time == 100)
        .expect("zscore fixture should emit prediction at 100");

    assert_eq!(leaked.signal_value, 1);
    assert_eq!(leaked.max_input_received_time, 120);
    assert_eq!(output.predictions.impossible_predictions().len(), 1);
}

#[test]
fn alfred_fixture_blocks_same_day_vintage_until_received() {
    let events = alfred_events();
    let output = ReplayEngine::with_signal(ZScoreTestSignal)
        .replay_with_order(&events, ReplayOptions::default(), ReplayOrder::ReceivedTime)
        .unwrap();
    let blocked_input = events
        .iter()
        .find(|event| event.event_id == "dgs10_20200318_v20200319")
        .expect("ALFRED fixture should contain the next-day DGS10 vintage")
        .event_key;
    let prediction_event_key = events
        .iter()
        .find(|event| event.event_id == "p_20200318_close_before_vintage")
        .expect("ALFRED fixture should contain the 2020-03-18 close prediction")
        .event_key;

    let prediction = output
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_event_key == prediction_event_key)
        .expect("received-time replay should emit the 2020-03-18 prediction");

    assert_eq!(prediction.prediction_time, 202003181600);
    assert_eq!(prediction.max_input_received_time, 202003180900);
    assert_eq!(prediction.max_input_received_sequence_number, 7);
    assert!(!prediction.input_event_ids_used.contains_key(blocked_input));
}

#[test]
fn observed_time_baseline_leaks_alfred_same_day_vintage() {
    let events = alfred_events();
    let output = ReplayEngine::with_signal(ZScoreTestSignal)
        .replay_with_order(
            &events,
            ReplayOptions::default(),
            ReplayOrder::ObservedTimeLeaky,
        )
        .unwrap();
    let same_day_vintage = events
        .iter()
        .find(|event| event.event_id == "dgs10_20200318_v20200319")
        .expect("ALFRED fixture should contain the same-day DGS10 vintage")
        .event_key;

    let leaked = output
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_time == 202003181600)
        .expect("ALFRED fixture should emit the 2020-03-18 prediction");

    assert_eq!(leaked.max_input_received_time, 202003190900);
    assert_eq!(leaked.max_input_received_sequence_number, 9);
    assert!(leaked.input_event_ids_used.contains_key(same_day_vintage));
    assert!(leaked.max_input_received_time > leaked.prediction_time);
}

#[test]
fn alfred_payems_revision_is_not_used_before_received() {
    let events = alfred_payems_revision_events();
    let output = ReplayEngine::with_signal(ZScoreTestSignal)
        .replay_with_order(&events, ReplayOptions::default(), ReplayOrder::ReceivedTime)
        .unwrap();
    let correction = events
        .iter()
        .find(|event| event.event_id == "payems_20190101_v20200301_revision")
        .expect("PAYEMS fixture should contain the revised ALFRED vintage")
        .event_key;
    let prediction_key = events
        .iter()
        .find(|event| event.event_id == "p_after_initial_before_revision")
        .expect("PAYEMS fixture should contain the pre-revision prediction")
        .event_key;

    let prediction = output
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_event_key == prediction_key)
        .expect("received-time replay should emit the pre-revision prediction");

    assert_eq!(prediction.prediction_time, 202002141600);
    assert_eq!(prediction.max_input_received_time, 202002010900);
    assert!(!prediction.input_event_ids_used.contains_key(correction));
}

#[test]
fn observed_time_baseline_leaks_alfred_payems_revision() {
    let events = alfred_payems_revision_events();
    let output = ReplayEngine::with_signal(ZScoreTestSignal)
        .replay_with_order(
            &events,
            ReplayOptions::default(),
            ReplayOrder::ObservedTimeLeaky,
        )
        .unwrap();
    let correction = events
        .iter()
        .find(|event| event.event_id == "payems_20190101_v20200301_revision")
        .expect("PAYEMS fixture should contain the revised ALFRED vintage")
        .event_key;

    let leaked = output
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_time == 202002141600)
        .expect("PAYEMS fixture should emit the pre-revision prediction");

    assert_eq!(leaked.max_input_received_time, 202003010900);
    assert!(leaked.input_event_ids_used.contains_key(correction));
    assert!(leaked.max_input_received_time > leaked.prediction_time);
}

#[test]
fn alfred_payems_large_revision_fixture_exercises_many_real_corrections() {
    let events = alfred_payems_large_revision_events();
    let feature_corrections = events
        .iter()
        .filter(|event| event.role == EventRole::FeatureCorrection)
        .count();
    let predictions = events
        .iter()
        .filter(|event| event.role == EventRole::Prediction)
        .count();

    assert_eq!(events.len(), 133);
    assert_eq!(feature_corrections, 76);
    assert_eq!(predictions, 23);

    let strict = ReplayEngine::with_signal(ZScoreTestSignal)
        .replay_with_order(&events, ReplayOptions::default(), ReplayOrder::ReceivedTime)
        .unwrap();

    assert!(strict.predictions.impossible_predictions().is_empty());

    let observed_time = ReplayEngine::with_signal(ZScoreTestSignal)
        .replay_with_order(
            &events,
            ReplayOptions::default(),
            ReplayOrder::ObservedTimeLeaky,
        )
        .unwrap();

    assert_eq!(observed_time.predictions.impossible_predictions().len(), 22);
}
