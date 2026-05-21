use crate::catalog::SymbolCatalog;
use crate::ids::SymbolSlot;
use crate::state::StateStore;
use crate::{
    feature_recipe_hash, Event, EventRole, ParseEventError, PredictionLog, PredictionRecord, Signal,
};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Options controlling non-prediction replay side effects.
pub struct ReplayOptions {
    /// Whether outcome rows should be counted after predictions are emitted.
    pub compute_outcomes: bool,
}

impl Default for ReplayOptions {
    fn default() -> Self {
        Self {
            compute_outcomes: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Result returned by a replay run.
pub struct ReplayOutput {
    /// Append-only prediction log produced during replay.
    pub predictions: PredictionLog,
    /// Number of outcome rows observed when outcome computation is enabled.
    pub outcomes_seen: usize,
    /// Number of events replayed after deterministic ordering.
    pub replayed_events: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Event ordering mode used by the replay engine.
pub enum ReplayOrder {
    /// Correct received-time replay ordering.
    ReceivedTime,
    /// Deliberately leaky observed-time baseline for negative controls.
    ObservedTimeLeaky,
}

#[derive(Debug)]
/// Deterministic as-of replay engine parameterized by a signal.
pub struct ReplayEngine<S> {
    signal: S,
}

impl<S: Signal> ReplayEngine<S> {
    /// Creates a replay engine with a custom signal.
    pub fn with_signal(signal: S) -> Self {
        Self { signal }
    }

    /// Replays events in received-time order.
    pub fn replay(
        &self,
        events: &[Event],
        options: ReplayOptions,
    ) -> Result<ReplayOutput, ReplayError> {
        self.replay_with_order(events, options, ReplayOrder::ReceivedTime)
    }

    /// Replays events with an explicit ordering mode.
    pub fn replay_with_order(
        &self,
        events: &[Event],
        options: ReplayOptions,
        order: ReplayOrder,
    ) -> Result<ReplayOutput, ReplayError> {
        validate_event_identity(events)?;

        let mut ordered = events.to_vec();
        match order {
            ReplayOrder::ReceivedTime => {
                ordered.sort_by(|left, right| left.replay_key().cmp(&right.replay_key()));
            }
            ReplayOrder::ObservedTimeLeaky => {
                ordered.sort_by(|left, right| left.observed_key().cmp(&right.observed_key()));
            }
        }

        let mut symbol_catalog = SymbolCatalog::new();
        let slotted_order = ordered
            .iter()
            .map(|event| {
                Ok(SlottedEvent {
                    event,
                    symbol: symbol_catalog.intern_event(event)?,
                })
            })
            .collect::<Result<Vec<_>, ParseEventError>>()?;

        let mut state = StateStore::with_symbol_count(symbol_catalog.len());
        let mut predictions = PredictionLog::with_symbol_catalog(&ordered, &symbol_catalog);
        let mut outcomes_seen = 0;
        let signal_name = self.signal.name();
        let signal_config_descriptor = self.signal.config_descriptor();

        for slotted in &slotted_order {
            let event = slotted.event;
            match event.role {
                EventRole::Feature | EventRole::FeatureCorrection => {
                    state.writer().apply(event, slotted.symbol)?
                }
                EventRole::Prediction => {
                    let snapshot = self.signal.evaluate(
                        state.as_of_view(),
                        slotted.symbol,
                        event.received_time,
                    );
                    predictions.append(PredictionRecord {
                        prediction_event_key: event.event_key,
                        prediction_time: event.received_time,
                        prediction_received_sequence_number: event.received_sequence_number,
                        symbol: event.symbol_key,
                        signal_value: snapshot.signal_value,
                        input_event_ids_used: snapshot.input_event_ids_used,
                        max_input_received_time: snapshot.max_input_received_time,
                        max_input_received_sequence_number: snapshot
                            .max_input_received_sequence_number,
                        max_input_event_key: snapshot.max_input_event_key,
                        feature_recipe_hash: snapshot.feature_recipe_hash.unwrap_or_else(|| {
                            feature_recipe_hash(
                                signal_name,
                                &signal_config_descriptor,
                                snapshot.input_event_ids_used,
                            )
                        }),
                    });
                }
                EventRole::Outcome => {
                    if options.compute_outcomes {
                        outcomes_seen += 1;
                    }
                }
            }
        }

        Ok(ReplayOutput {
            predictions,
            outcomes_seen,
            replayed_events: ordered.len(),
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct SlottedEvent<'a> {
    event: &'a Event,
    symbol: SymbolSlot,
}

fn validate_event_identity(events: &[Event]) -> Result<(), ReplayError> {
    let mut event_ids = BTreeSet::new();
    let mut event_keys = BTreeMap::new();
    let mut receipt_positions = BTreeMap::new();

    for event in events {
        if !event_ids.insert(event.event_id.as_str()) {
            return Err(ParseEventError::DuplicateEventId {
                event_id: event.event_id.clone(),
            }
            .into());
        }

        if let Some(first_event_id) = event_keys.insert(event.event_key, event.event_id.as_str()) {
            if first_event_id != event.event_id.as_str() {
                return Err(ParseEventError::EventKeyCollision {
                    first_event_id: first_event_id.to_string(),
                    second_event_id: event.event_id.clone(),
                }
                .into());
            }
        }

        let receipt_position = (event.received_time, event.received_sequence_number);
        if let Some(first_event_id) =
            receipt_positions.insert(receipt_position, event.event_id.as_str())
        {
            return Err(ParseEventError::DuplicateReceivedSequenceNumber {
                received_time: event.received_time,
                received_sequence_number: event.received_sequence_number,
                first_event_id: first_event_id.to_string(),
                second_event_id: event.event_id.clone(),
            }
            .into());
        }
    }

    Ok(())
}

/// Parses newline-delimited pipe records into events.
pub fn parse_pipe_events(input: &str) -> Result<Vec<Event>, ReplayError> {
    let mut events = Vec::new();

    for (index, line) in input.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let event = Event::from_pipe_record(trimmed).map_err(|source| ReplayError {
            line: Some(index + 1),
            source,
        })?;
        events.push(event);
    }

    Ok(events)
}

#[derive(Debug)]
/// Error returned while parsing or replaying an event stream.
pub struct ReplayError {
    /// One-based input line number when parsing failed on a specific line.
    pub line: Option<usize>,
    /// Underlying parse or replay error.
    pub source: ParseEventError,
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "failed to parse or replay line {line}: {}", self.source),
            None => write!(f, "failed to replay events: {}", self.source),
        }
    }
}

impl Error for ReplayError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

impl From<ParseEventError> for ReplayError {
    fn from(source: ParseEventError) -> Self {
        Self { line: None, source }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AsOfView, FixedDecimal, SignalEvaluation, SymbolSlot, FIXED_DECIMAL_SCALE};

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

    #[test]
    fn ignores_comments_and_blank_lines() {
        let input = "\n# comment\np1|1|1|1|prediction|AAPL|\n";
        let events = parse_pipe_events(input).unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn replays_by_received_time_not_file_order() {
        let input = "\
p1|580|580|3|prediction|AAPL|
n1|572|585|2|feature|AAPL|sentiment=positive
p2|590|590|4|prediction|AAPL|
";
        let events = parse_pipe_events(input).unwrap();
        let output = ReplayEngine::with_signal(LastFeatureTestSignal)
            .replay(&events, ReplayOptions::default())
            .unwrap();
        let records = output.predictions.records();

        assert_eq!(records[0].signal_value, 0);
        assert_eq!(records[1].signal_value, 1);
        let n1 = events
            .iter()
            .find(|event| event.event_id == "n1")
            .expect("fixture should contain n1");
        assert!(records[1].input_event_ids_used.contains_key(n1.event_key));
    }

    #[test]
    fn dense_symbol_slots_keep_symbol_state_isolated() {
        let input = "\
a1|100|100|1|feature|AAPL|sentiment=positive
m1|105|105|2|feature|MSFT|sentiment=negative
pa|110|110|3|prediction|AAPL|
pm|115|115|4|prediction|MSFT|
";
        let events = parse_pipe_events(input).unwrap();
        let output = ReplayEngine::with_signal(LastFeatureTestSignal)
            .replay(&events, ReplayOptions::default())
            .unwrap();
        let records = output.predictions.records();

        assert_eq!(records[0].symbol, events[0].symbol_key);
        assert_eq!(records[0].signal_value, 1);
        assert_eq!(records[1].symbol, events[1].symbol_key);
        assert_eq!(records[1].signal_value, -1);
    }

    #[test]
    fn replay_rejects_symbol_id_collisions_before_state_updates() {
        let mut events = vec![
            Event::new(
                "a1",
                100,
                100,
                1,
                EventRole::Feature,
                "AAPL",
                "sentiment=positive",
            ),
            Event::new(
                "m1",
                105,
                105,
                2,
                EventRole::Feature,
                "MSFT",
                "sentiment=negative",
            ),
        ];
        events[1].symbol_key = events[0].symbol_key;

        let error = ReplayEngine::with_signal(LastFeatureTestSignal)
            .replay(&events, ReplayOptions::default())
            .unwrap_err();

        assert!(matches!(
            error.source,
            ParseEventError::SymbolIdCollision {
                existing_symbol,
                conflicting_symbol,
                ..
            } if existing_symbol == "AAPL" && conflicting_symbol == "MSFT"
        ));
    }

    #[test]
    fn rejects_duplicate_event_ids() {
        let events = parse_pipe_events(
            "\
e1|100|100|1|feature|XYZ|sentiment=positive
e1|110|110|2|prediction|XYZ|
",
        )
        .unwrap();

        let error = ReplayEngine::with_signal(LastFeatureTestSignal)
            .replay(&events, ReplayOptions::default())
            .unwrap_err();

        assert!(matches!(
            error.source,
            ParseEventError::DuplicateEventId { ref event_id } if event_id == "e1"
        ));
    }

    #[test]
    fn rejects_duplicate_received_sequence_numbers_at_same_received_time() {
        let events = parse_pipe_events(
            "\
f1|100|100|1|feature|XYZ|sentiment=positive
p1|110|100|1|prediction|XYZ|
",
        )
        .unwrap();

        let error = ReplayEngine::with_signal(LastFeatureTestSignal)
            .replay(&events, ReplayOptions::default())
            .unwrap_err();

        assert!(matches!(
            error.source,
            ParseEventError::DuplicateReceivedSequenceNumber {
                received_time: 100,
                received_sequence_number: 1,
                ..
            }
        ));
    }

    #[test]
    fn windowed_signal_records_multiple_inputs() {
        let input = "\
f1|100|100|1|feature|XYZ|sentiment=positive
f2|110|110|2|feature|XYZ|sentiment=negative
f3|120|120|3|feature|XYZ|sentiment=positive
p1|130|130|4|prediction|XYZ|
";
        let events = parse_pipe_events(input).unwrap();
        let output = ReplayEngine::with_signal(WindowedFeatureTestSignal { window: 5 })
            .replay(&events, ReplayOptions::default())
            .unwrap();
        let record = &output.predictions.records()[0];

        assert_eq!(record.signal_value, 1);
        assert_eq!(record.input_event_ids_used.len(), 3);
        assert_eq!(record.max_input_received_time, 120);
        assert_eq!(record.max_input_received_sequence_number, 3);
        for event in events
            .iter()
            .filter(|event| event.role.updates_signal_state())
        {
            assert!(record.input_event_ids_used.contains_key(event.event_key));
        }
    }

    #[test]
    fn windowed_zscore_signal_records_numeric_inputs() {
        let input = "\
px1|100|100|1|feature|XYZ|score=10
px2|110|110|2|feature|XYZ|score=10
px3|120|120|3|feature|XYZ|score=10
px4|130|130|4|feature|XYZ|score=30
p1|140|140|5|prediction|XYZ|
";
        let events = parse_pipe_events(input).unwrap();
        let output = ReplayEngine::with_signal(ZScoreTestSignal)
            .replay(&events, ReplayOptions::default())
            .unwrap();
        let record = &output.predictions.records()[0];

        assert_eq!(record.signal_value, 1);
        assert_eq!(record.input_event_ids_used.len(), 4);
        assert_eq!(record.max_input_received_time, 130);
        assert_eq!(record.max_input_received_sequence_number, 4);
    }

    #[test]
    fn vol_adjusted_momentum_records_crossover_input_provenance() {
        let input = "\
px1|100|100|1|feature|XYZ|score=10
px2|110|110|2|feature|XYZ|score=10
px3|120|120|3|feature|XYZ|score=10
px4|130|130|4|feature|XYZ|score=30
p1|140|140|5|prediction|XYZ|
";
        let events = parse_pipe_events(input).unwrap();
        let output = ReplayEngine::with_signal(VolAdjustedMomentumTestSignal)
            .replay(&events, ReplayOptions::default())
            .unwrap();
        let record = &output.predictions.records()[0];

        assert_eq!(record.signal_value, 1);
        assert_eq!(record.input_event_ids_used.len(), 4);
        assert_eq!(record.max_input_received_time, 130);
        assert_eq!(record.max_input_received_sequence_number, 4);
    }

    #[test]
    fn vol_adjusted_momentum_returns_zero_on_flat_series() {
        let input = "\
px1|100|100|1|feature|XYZ|score=10
px2|110|110|2|feature|XYZ|score=10
px3|120|120|3|feature|XYZ|score=10
px4|130|130|4|feature|XYZ|score=10
p1|140|140|5|prediction|XYZ|
";
        let events = parse_pipe_events(input).unwrap();
        let output = ReplayEngine::with_signal(VolAdjustedMomentumTestSignal)
            .replay(&events, ReplayOptions::default())
            .unwrap();
        let record = &output.predictions.records()[0];

        assert_eq!(record.signal_value, 0);
        assert_eq!(record.input_event_ids_used.len(), 4);
        assert_eq!(record.max_input_received_time, 130);
        assert_eq!(record.max_input_received_sequence_number, 4);
    }

    #[test]
    fn recipe_hash_includes_signal_config_descriptor() {
        let input = "\
f1|100|100|1|feature|XYZ|sentiment=positive
p1|110|110|2|prediction|XYZ|
";
        let events = parse_pipe_events(input).unwrap();
        let last_feature = ReplayEngine::with_signal(LastFeatureTestSignal)
            .replay(&events, ReplayOptions::default())
            .unwrap();
        let windowed = ReplayEngine::with_signal(WindowedFeatureTestSignal { window: 5 })
            .replay(&events, ReplayOptions::default())
            .unwrap();

        assert_ne!(
            last_feature.predictions.records()[0].feature_recipe_hash,
            windowed.predictions.records()[0].feature_recipe_hash
        );
    }
}
