use crate::state::StateStore;
use crate::{
    Event, EventRole, LastFeatureSentimentSignal, ParseEventError, PredictionLog, PredictionRecord,
    Signal,
};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayOptions {
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
pub struct ReplayOutput {
    pub predictions: PredictionLog,
    pub outcomes_seen: usize,
    pub replayed_events: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayOrder {
    ReceivedTime,
    ObservedTimeLeaky,
}

#[derive(Debug, Default)]
pub struct ReplayEngine<S = LastFeatureSentimentSignal> {
    signal: S,
}

impl ReplayEngine<LastFeatureSentimentSignal> {
    pub fn new() -> Self {
        Self {
            signal: LastFeatureSentimentSignal,
        }
    }
}

impl<S: Signal> ReplayEngine<S> {
    pub fn with_signal(signal: S) -> Self {
        Self { signal }
    }

    pub fn replay(
        &self,
        events: &[Event],
        options: ReplayOptions,
    ) -> Result<ReplayOutput, ReplayError> {
        self.replay_with_order(events, options, ReplayOrder::ReceivedTime)
    }

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

        let mut state = StateStore::new();
        let mut predictions = PredictionLog::with_event_catalog(&ordered);
        let mut outcomes_seen = 0;

        for event in &ordered {
            match event.role {
                EventRole::Feature | EventRole::FeatureCorrection => state.writer().apply(event)?,
                EventRole::Prediction => {
                    let snapshot = self.signal.predict(
                        state.as_of_view(),
                        event.symbol_key,
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
pub struct ReplayError {
    pub line: Option<usize>,
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
    use crate::WindowedFeatureSentimentSignal;

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
}
