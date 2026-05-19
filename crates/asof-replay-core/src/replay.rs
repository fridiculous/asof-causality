use crate::state::StateStore;
use crate::{
    Event, EventKind, LastSentimentSignal, ParseEventError, PredictionLog, PredictionRecord, Signal,
};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayOptions {
    pub compute_labels: bool,
}

impl Default for ReplayOptions {
    fn default() -> Self {
        Self {
            compute_labels: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayOutput {
    pub predictions: PredictionLog,
    pub labels_seen: usize,
    pub replayed_events: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayOrder {
    ReceivedTime,
    ObservedTimeLeaky,
}

#[derive(Debug, Default)]
pub struct ReplayEngine<S = LastSentimentSignal> {
    signal: S,
}

impl ReplayEngine<LastSentimentSignal> {
    pub fn new() -> Self {
        Self {
            signal: LastSentimentSignal,
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
        let mut labels_seen = 0;

        for event in &ordered {
            match event.kind {
                EventKind::News | EventKind::Correction => state.writer().apply(event)?,
                EventKind::Predict => {
                    let snapshot =
                        self.signal
                            .predict(state.as_of_view(), &event.symbol, event.received_time);
                    predictions.append(PredictionRecord {
                        prediction_time: event.received_time,
                        prediction_sequence: event.sequence,
                        symbol: event.symbol.clone(),
                        signal_value: snapshot.signal_value,
                        input_event_ids_used: snapshot.input_event_ids_used,
                        max_input_received_time: snapshot.max_input_received_time,
                        max_input_sequence: snapshot.max_input_sequence,
                    });
                }
                EventKind::Label => {
                    if options.compute_labels {
                        labels_seen += 1;
                    }
                }
            }
        }

        Ok(ReplayOutput {
            predictions,
            labels_seen,
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

    #[test]
    fn ignores_comments_and_blank_lines() {
        let input = "\n# comment\np1|1|1|1|predict|AAPL|\n";
        let events = parse_pipe_events(input).unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn replays_by_received_time_not_file_order() {
        let input = "\
p1|580|580|3|predict|AAPL|
n1|572|585|2|news|AAPL|sentiment=positive
p2|590|590|4|predict|AAPL|
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
}
