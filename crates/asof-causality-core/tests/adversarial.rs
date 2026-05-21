use asof_causality_core::{
    parse_pipe_events, run_adversarial_checks, ReplayEngine, ReplayOptions, ReplayOrder,
    VolAdjustedMomentumSignal, WindowedFeatureSentimentSignal, WindowedZScoreSignal,
};

fn fixture_events() -> Vec<asof_causality_core::Event> {
    parse_pipe_events(include_str!("../../../examples/late-arrival.pipe")).unwrap()
}

fn negative_control_events() -> Vec<asof_causality_core::Event> {
    parse_pipe_events(include_str!(
        "../../../examples/lookahead-negative-control.pipe"
    ))
    .unwrap()
}

fn zscore_events() -> Vec<asof_causality_core::Event> {
    parse_pipe_events(include_str!("../../../examples/zscore-lookahead.pipe")).unwrap()
}

fn alfred_events() -> Vec<asof_causality_core::Event> {
    parse_pipe_events(include_str!("../../../examples/alfred-dgs10-sp500.pipe")).unwrap()
}

fn assert_check_passes(name: &str) {
    let events = fixture_events();
    let report = run_adversarial_checks(&events);
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
    let with_outcomes = ReplayEngine::new()
        .replay(
            &events,
            ReplayOptions {
                compute_outcomes: true,
            },
        )
        .unwrap()
        .predictions
        .transcript_hash();
    let without_outcomes = ReplayEngine::new()
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
    let output = ReplayEngine::new()
        .replay_with_order(&events, ReplayOptions::default(), ReplayOrder::ReceivedTime)
        .unwrap();

    assert!(output.predictions.impossible_predictions().is_empty());
}

#[test]
fn observed_time_baseline_leaks_on_negative_control() {
    let events = negative_control_events();
    let output = ReplayEngine::new()
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
    let output = ReplayEngine::with_signal(WindowedFeatureSentimentSignal::new(5))
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
    let report = asof_causality_core::run_adversarial_checks_with_options_for_signal(
        &events,
        asof_causality_core::CheckOptions::exhaustive(),
        WindowedZScoreSignal::new(),
    );

    assert!(report.passed(), "{report:?}");
}

#[test]
fn observed_time_baseline_leaks_numeric_zscore_input() {
    let events = zscore_events();
    let output = ReplayEngine::with_signal(WindowedZScoreSignal::new())
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
    let report = asof_causality_core::run_adversarial_checks_with_options_for_signal(
        &events,
        asof_causality_core::CheckOptions::exhaustive(),
        VolAdjustedMomentumSignal::new(),
    );

    assert!(report.passed(), "{report:?}");
}

#[test]
fn observed_time_baseline_leaks_vol_adjusted_momentum_input() {
    let events = zscore_events();
    let output = ReplayEngine::with_signal(VolAdjustedMomentumSignal::new())
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
    let output = ReplayEngine::with_signal(WindowedZScoreSignal::new())
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
    let output = ReplayEngine::with_signal(WindowedZScoreSignal::new())
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
