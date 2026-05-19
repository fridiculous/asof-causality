use asof_replay_core::{
    generate_events, parse_pipe_events, run_adversarial_checks,
    run_universal_leakage_checks_with_options, CheckOptions, Event, EventKind, GenerateConfig,
    ReplayEngine, ReplayOptions, ReplayOrder, Scenario,
};
use proptest::prelude::*;

fn fixture_events() -> Vec<asof_replay_core::Event> {
    parse_pipe_events(include_str!("../../../examples/late-arrival.pipe")).unwrap()
}

fn negative_control_events() -> Vec<asof_replay_core::Event> {
    parse_pipe_events(include_str!(
        "../../../examples/lookahead-negative-control.pipe"
    ))
    .unwrap()
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
fn corrections_are_append_only() {
    assert_check_passes("correction_append_only");
}

#[test]
fn labels_do_not_affect_predictions() {
    assert_check_passes("label_separation");
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
fn disabling_label_computation_does_not_change_transcript_hash() {
    let events = fixture_events();
    let with_labels = ReplayEngine::new()
        .replay(
            &events,
            ReplayOptions {
                compute_labels: true,
            },
        )
        .unwrap()
        .predictions
        .transcript_hash();
    let without_labels = ReplayEngine::new()
        .replay(
            &events,
            ReplayOptions {
                compute_labels: false,
            },
        )
        .unwrap()
        .predictions
        .transcript_hash();

    assert_eq!(with_labels, without_labels);
}

#[test]
fn received_time_engine_survives_negative_control() {
    let events = negative_control_events();
    let output = ReplayEngine::new()
        .replay_with_order(&events, ReplayOptions::default(), ReplayOrder::ReceivedTime)
        .unwrap();

    assert!(output
        .predictions
        .records()
        .iter()
        .all(|record| !record.uses_future_input()));
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

    let late_news_leak = output
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_time == 120)
        .expect("negative fixture should emit prediction at 120");

    assert_eq!(late_news_leak.max_input_received_time, 150);
    assert!(late_news_leak.uses_future_input());

    let correction_leak = output
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_time == 170)
        .expect("negative fixture should emit prediction at 170");

    assert_eq!(correction_leak.max_input_received_time, 180);
    assert!(correction_leak.uses_future_input());
}

#[test]
fn sequence_order_is_part_of_knowability() {
    let events = parse_pipe_events(
        "\
n_same_time_late|90|100|3|news|AAPL|sentiment=positive
p_before_same_time|100|100|2|predict|AAPL|
",
    )
    .unwrap();

    let received_time = ReplayEngine::new()
        .replay_with_order(&events, ReplayOptions::default(), ReplayOrder::ReceivedTime)
        .unwrap();
    let correct_prediction = received_time
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_time == 100 && record.prediction_sequence == 2)
        .expect("fixture should emit same-time prediction");

    assert_eq!(correct_prediction.signal_value, 0);
    assert!(!correct_prediction.uses_future_input());

    let observed_time = ReplayEngine::new()
        .replay_with_order(
            &events,
            ReplayOptions::default(),
            ReplayOrder::ObservedTimeLeaky,
        )
        .unwrap();
    let leaky_prediction = observed_time
        .predictions
        .records()
        .iter()
        .find(|record| record.prediction_time == 100 && record.prediction_sequence == 2)
        .expect("fixture should emit same-time prediction");

    assert_eq!(leaky_prediction.signal_value, 1);
    assert_eq!(leaky_prediction.max_input_received_time, 100);
    assert_eq!(leaky_prediction.max_input_sequence, 3);
    assert!(leaky_prediction.uses_future_input());
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    #[test]
    fn universal_leakage_checks_hold_for_random_bitemporal_streams(
        events in arb_bitemporal_stream(128)
    ) {
        let report = run_universal_leakage_checks_with_options(
            &events,
            CheckOptions::sampled(8),
        );

        prop_assert!(report.passed(), "{report:?}");
    }

    #[test]
    fn late_heavy_generator_has_multiple_late_contrast_opportunities(seed in any::<u64>()) {
        let config = GenerateConfig {
            seed,
            events: 512,
            symbols: 16,
            ..GenerateConfig::for_scenario(Scenario::LateHeavy)
        };
        let generated = generate_events(&config);
        let opportunities = late_contrast_opportunities(&generated.events);

        prop_assert!(
            opportunities >= 3,
            "expected at least 3 late contrast opportunities, found {opportunities}"
        );
    }
}

fn arb_bitemporal_stream(max_events: usize) -> impl Strategy<Value = Vec<Event>> {
    prop::collection::vec(
        (
            0_u64..10_000,
            0_u64..500,
            0_u8..4,
            0_u8..16,
            0_u8..3,
            any::<i16>(),
        ),
        0..=max_events,
    )
    .prop_map(|rows| {
        rows.into_iter()
            .enumerate()
            .map(
                |(
                    index,
                    (observed_time, lag, kind_index, symbol_index, sentiment_index, label),
                )| {
                    let kind = match kind_index {
                        0 => EventKind::News,
                        1 => EventKind::Correction,
                        2 => EventKind::Predict,
                        _ => EventKind::Label,
                    };
                    let payload = match kind {
                        EventKind::News => format!("sentiment={}", sentiment(sentiment_index)),
                        EventKind::Correction => {
                            format!(
                                "sentiment={},corrects=e{}",
                                sentiment(sentiment_index),
                                index.saturating_sub(1)
                            )
                        }
                        EventKind::Predict => String::new(),
                        EventKind::Label => format!("return_bps={label}"),
                    };

                    Event::new(
                        format!("e{index}"),
                        observed_time,
                        observed_time + lag,
                        index as u64 + 1,
                        kind,
                        format!("SYM{:02}", symbol_index),
                        payload,
                    )
                },
            )
            .collect()
    })
}

fn sentiment(index: u8) -> &'static str {
    match index % 3 {
        0 => "negative",
        1 => "neutral",
        _ => "positive",
    }
}

fn late_contrast_opportunities(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| event.kind.updates_prediction_state())
        .filter(|update| {
            events.iter().any(|prediction| {
                prediction.kind == EventKind::Predict
                    && prediction.symbol == update.symbol
                    && update.observed_time <= prediction.received_time
                    && prediction.received_time < update.received_time
            })
        })
        .count()
}
