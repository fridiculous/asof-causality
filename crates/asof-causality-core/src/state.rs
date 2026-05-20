use crate::{Event, EventKey, InputSet, Sentiment, SymbolId, MAX_INPUTS_PER_PREDICTION};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolSnapshot {
    pub signal_value: i8,
    pub input_event_ids_used: InputSet,
    pub max_input_received_time: u64,
    pub max_input_sequence: u64,
    pub max_input_event_key: Option<EventKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SymbolState {
    recent: [FeatureObservation; MAX_INPUTS_PER_PREDICTION],
    recent_len: usize,
}

impl SymbolState {
    fn new() -> Self {
        Self {
            recent: [FeatureObservation::default(); MAX_INPUTS_PER_PREDICTION],
            recent_len: 0,
        }
    }

    fn push(&mut self, observation: FeatureObservation) {
        if self.recent_len < MAX_INPUTS_PER_PREDICTION {
            self.recent[self.recent_len] = observation;
            self.recent_len += 1;
            return;
        }

        self.recent.copy_within(1.., 0);
        self.recent[MAX_INPUTS_PER_PREDICTION - 1] = observation;
    }

    fn latest(&self) -> Option<FeatureObservation> {
        self.recent_len
            .checked_sub(1)
            .map(|index| self.recent[index])
    }

    fn recent_window(&self, window: usize) -> &[FeatureObservation] {
        let count = window.min(self.recent_len).min(MAX_INPUTS_PER_PREDICTION);
        let start = self.recent_len - count;
        &self.recent[start..self.recent_len]
    }
}

impl Default for SymbolState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FeatureObservation {
    sentiment: Sentiment,
    input_key: EventKey,
    received_time: u64,
    sequence: u64,
}

impl Default for FeatureObservation {
    fn default() -> Self {
        Self {
            sentiment: Sentiment::Neutral,
            input_key: EventKey::default(),
            received_time: 0,
            sequence: 0,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct StateStore {
    by_symbol: BTreeMap<SymbolId, SymbolState>,
}

impl StateStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn writer(&mut self) -> StateWriter<'_> {
        StateWriter { store: self }
    }

    pub(crate) fn as_of_view(&self) -> AsOfView<'_> {
        AsOfView { store: self }
    }
}

pub(crate) struct StateWriter<'a> {
    store: &'a mut StateStore,
}

impl StateWriter<'_> {
    pub(crate) fn apply(&mut self, event: &Event) -> Result<(), crate::ParseEventError> {
        let Some(sentiment) = event.sentiment()? else {
            return Ok(());
        };

        self.store
            .by_symbol
            .entry(event.symbol_key)
            .or_default()
            .push(FeatureObservation {
                sentiment,
                input_key: event.event_key,
                received_time: event.received_time,
                sequence: event.sequence,
            });

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AsOfView<'a> {
    store: &'a StateStore,
}

impl AsOfView<'_> {
    pub fn snapshot(&self, symbol: SymbolId) -> SymbolSnapshot {
        match self.store.by_symbol.get(&symbol) {
            Some(state) => match state.latest() {
                Some(observation) => SymbolSnapshot {
                    signal_value: observation.sentiment.signal_value(),
                    input_event_ids_used: InputSet::one(observation.input_key),
                    max_input_received_time: observation.received_time,
                    max_input_sequence: observation.sequence,
                    max_input_event_key: Some(observation.input_key),
                },
                None => empty_snapshot(),
            },
            None => empty_snapshot(),
        }
    }

    pub fn windowed_snapshot(&self, symbol: SymbolId, window: usize) -> SymbolSnapshot {
        let Some(state) = self.store.by_symbol.get(&symbol) else {
            return empty_snapshot();
        };

        let observations = state.recent_window(window);
        if observations.is_empty() {
            return empty_snapshot();
        }

        let mut keys = [EventKey::default(); MAX_INPUTS_PER_PREDICTION];
        let mut signal_sum = 0_i16;
        let mut max_observation = observations[0];

        for (index, observation) in observations.iter().enumerate() {
            keys[index] = observation.input_key;
            signal_sum += i16::from(observation.sentiment.signal_value());
            if (
                observation.received_time,
                observation.sequence,
                observation.input_key,
            ) > (
                max_observation.received_time,
                max_observation.sequence,
                max_observation.input_key,
            ) {
                max_observation = *observation;
            }
        }

        SymbolSnapshot {
            signal_value: signal_sum.signum() as i8,
            input_event_ids_used: InputSet::from_ordered_keys(&keys[..observations.len()]),
            max_input_received_time: max_observation.received_time,
            max_input_sequence: max_observation.sequence,
            max_input_event_key: Some(max_observation.input_key),
        }
    }
}

fn empty_snapshot() -> SymbolSnapshot {
    SymbolSnapshot {
        signal_value: 0,
        input_event_ids_used: InputSet::empty(),
        max_input_received_time: 0,
        max_input_sequence: 0,
        max_input_event_key: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventRole;

    #[test]
    fn windowed_snapshot_records_multiple_feature_inputs() {
        let mut store = StateStore::new();
        let events = [
            Event::new(
                "f1",
                10,
                10,
                1,
                EventRole::Feature,
                "XYZ",
                "sentiment=positive",
            ),
            Event::new(
                "f2",
                20,
                20,
                2,
                EventRole::Feature,
                "XYZ",
                "sentiment=negative",
            ),
            Event::new(
                "f3",
                30,
                30,
                3,
                EventRole::Feature,
                "XYZ",
                "sentiment=positive",
            ),
        ];

        for event in &events {
            store.writer().apply(event).unwrap();
        }

        let snapshot = store
            .as_of_view()
            .windowed_snapshot(events[0].symbol_key, 3);

        assert_eq!(snapshot.signal_value, 1);
        assert_eq!(snapshot.input_event_ids_used.len(), 3);
        assert_eq!(snapshot.max_input_event_key, Some(events[2].event_key));
        assert_eq!(
            snapshot.input_event_ids_used.iter().collect::<Vec<_>>(),
            events
                .iter()
                .map(|event| event.event_key)
                .collect::<Vec<_>>()
        );
        assert_eq!(snapshot.max_input_received_time, 30);
        assert_eq!(snapshot.max_input_sequence, 3);
    }
}
