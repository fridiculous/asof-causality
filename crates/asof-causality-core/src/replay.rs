use crate::catalog::SymbolCatalog;
use crate::ids::SymbolSlot;
use crate::state::StateStore;
use crate::{
    feature_recipe_hash, Event, EventRole, LastFeatureSentimentSignal, ParseEventError,
    PredictionLog, PredictionRecord, Signal,
};
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

#[derive(Debug, Default)]
/// Deterministic as-of replay engine parameterized by a signal.
pub struct ReplayEngine<S = LastFeatureSentimentSignal> {
    signal: S,
}

impl ReplayEngine<LastFeatureSentimentSignal> {
    /// Creates a replay engine with the default last-feature sentiment signal.
    pub fn new() -> Self {
        Self {
            signal: LastFeatureSentimentSignal,
        }
    }
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
                    let snapshot = self.signal.predict(
                        state.as_of_view(),
                        slotted.symbol,
                        event.received_time,
                    );
                    predictions.append(PredictionRecord {
                        prediction_event_key: event.event_key,
                        prediction_time: event.received_time,
                        prediction_sequence: event.sequence,
                        symbol: event.symbol_key,
                        signal_value: snapshot.signal_value,
                        input_event_ids_used: snapshot.input_event_ids_used,
                        max_input_received_time: snapshot.max_input_received_time,
                        max_input_sequence: snapshot.max_input_sequence,
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
    use crate::{WindowedFeatureSentimentSignal, WindowedZScoreSignal};

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
        let output = ReplayEngine::new()
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
        let output = ReplayEngine::new()
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

        let error = ReplayEngine::new()
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
    fn windowed_signal_records_multiple_inputs() {
        let input = "\
f1|100|100|1|feature|XYZ|sentiment=positive
f2|110|110|2|feature|XYZ|sentiment=negative
f3|120|120|3|feature|XYZ|sentiment=positive
p1|130|130|4|prediction|XYZ|
";
        let events = parse_pipe_events(input).unwrap();
        let output = ReplayEngine::with_signal(WindowedFeatureSentimentSignal::new(5))
            .replay(&events, ReplayOptions::default())
            .unwrap();
        let record = &output.predictions.records()[0];

        assert_eq!(record.signal_value, 1);
        assert_eq!(record.input_event_ids_used.len(), 3);
        assert_eq!(record.max_input_received_time, 120);
        assert_eq!(record.max_input_sequence, 3);
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
        let output = ReplayEngine::with_signal(WindowedZScoreSignal::new())
            .replay(&events, ReplayOptions::default())
            .unwrap();
        let record = &output.predictions.records()[0];

        assert_eq!(record.signal_value, 1);
        assert_eq!(record.input_event_ids_used.len(), 4);
        assert_eq!(record.max_input_received_time, 130);
        assert_eq!(record.max_input_sequence, 4);
    }

    #[test]
    fn recipe_hash_includes_signal_config_descriptor() {
        let input = "\
f1|100|100|1|feature|XYZ|sentiment=positive
p1|110|110|2|prediction|XYZ|
";
        let events = parse_pipe_events(input).unwrap();
        let last_feature = ReplayEngine::new()
            .replay(&events, ReplayOptions::default())
            .unwrap();
        let windowed = ReplayEngine::with_signal(WindowedFeatureSentimentSignal::new(5))
            .replay(&events, ReplayOptions::default())
            .unwrap();

        assert_ne!(
            last_feature.predictions.records()[0].feature_recipe_hash,
            windowed.predictions.records()[0].feature_recipe_hash
        );
    }
}
