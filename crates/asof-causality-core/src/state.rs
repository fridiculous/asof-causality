use crate::{
    Event, EventKey, FeatureRecipeHash, InputSet, Sentiment, SymbolId, MAX_INPUTS_PER_PREDICTION,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
/// Signal-visible state snapshot for one symbol at one prediction point.
pub struct SymbolSnapshot {
    /// Emitted prediction value.
    pub signal_value: i8,
    /// Input event keys used to produce the value.
    pub input_event_ids_used: InputSet,
    /// Maximum received time among input events used by the prediction.
    pub max_input_received_time: u64,
    /// Maximum sequence among input events at the maximum replay key.
    pub max_input_sequence: u64,
    /// Event key for the maximum input replay key, when an input exists.
    pub max_input_event_key: Option<EventKey>,
    /// Optional precomputed feature recipe hash supplied by a signal.
    pub feature_recipe_hash: Option<FeatureRecipeHash>,
}

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct FeatureObservation {
    sentiment: Option<Sentiment>,
    score: Option<f64>,
    input_key: EventKey,
    received_time: u64,
    sequence: u64,
}

#[derive(Debug, Default, Clone, PartialEq)]
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
        let Some(values) = event.feature_values()? else {
            return Ok(());
        };

        self.store
            .by_symbol
            .entry(event.symbol_key)
            .or_default()
            .push(FeatureObservation {
                sentiment: values.sentiment,
                score: values.score,
                input_key: event.event_key,
                received_time: event.received_time,
                sequence: event.sequence,
            });

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
/// Opaque read-only state view exposed to signal implementations.
pub struct AsOfView<'a> {
    store: &'a StateStore,
}

impl AsOfView<'_> {
    /// Returns the latest received sentiment snapshot for `symbol`.
    pub fn snapshot(&self, symbol: SymbolId) -> SymbolSnapshot {
        match self.store.by_symbol.get(&symbol) {
            Some(state) => match state
                .recent_window(MAX_INPUTS_PER_PREDICTION)
                .iter()
                .rev()
                .find(|observation| observation.sentiment.is_some())
                .copied()
            {
                Some(observation) => SymbolSnapshot {
                    signal_value: observation.sentiment.unwrap().signal_value(),
                    input_event_ids_used: InputSet::one(observation.input_key),
                    max_input_received_time: observation.received_time,
                    max_input_sequence: observation.sequence,
                    max_input_event_key: Some(observation.input_key),
                    feature_recipe_hash: None,
                },
                None => empty_snapshot(),
            },
            None => empty_snapshot(),
        }
    }

    /// Returns a bounded recent sentiment-window snapshot for `symbol`.
    pub fn windowed_snapshot(&self, symbol: SymbolId, window: usize) -> SymbolSnapshot {
        let Some(state) = self.store.by_symbol.get(&symbol) else {
            return empty_snapshot();
        };

        let observations = recent_observations_with_sentiment(state, window);
        if observations.is_empty() {
            return empty_snapshot();
        }

        let mut keys = [EventKey::default(); MAX_INPUTS_PER_PREDICTION];
        let mut signal_sum = 0_i16;
        let mut max_observation = observations[0];

        for (index, observation) in observations.iter().enumerate() {
            keys[index] = observation.input_key;
            signal_sum += i16::from(observation.sentiment.unwrap().signal_value());
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
            feature_recipe_hash: None,
        }
    }

    /// Returns a bounded recent numeric-score z-score snapshot for `symbol`.
    pub fn score_window_snapshot(
        &self,
        symbol: SymbolId,
        window: usize,
        threshold: f64,
    ) -> SymbolSnapshot {
        let Some(state) = self.store.by_symbol.get(&symbol) else {
            return empty_snapshot();
        };

        let observations = recent_observations_with_score(state, window);
        if observations.is_empty() {
            return empty_snapshot();
        }

        let mut keys = [EventKey::default(); MAX_INPUTS_PER_PREDICTION];
        let mut max_observation = observations[0];

        for (index, observation) in observations.iter().enumerate() {
            keys[index] = observation.input_key;
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

        let signal_value = zscore_signal_value(&observations, threshold);

        SymbolSnapshot {
            signal_value,
            input_event_ids_used: InputSet::from_ordered_keys(&keys[..observations.len()]),
            max_input_received_time: max_observation.received_time,
            max_input_sequence: max_observation.sequence,
            max_input_event_key: Some(max_observation.input_key),
            feature_recipe_hash: None,
        }
    }
}

fn recent_observations_with_sentiment(
    state: &SymbolState,
    window: usize,
) -> Vec<FeatureObservation> {
    recent_observations_matching(state, window, |observation| observation.sentiment.is_some())
}

fn recent_observations_with_score(state: &SymbolState, window: usize) -> Vec<FeatureObservation> {
    recent_observations_matching(state, window, |observation| observation.score.is_some())
}

fn recent_observations_matching(
    state: &SymbolState,
    window: usize,
    predicate: impl Fn(&FeatureObservation) -> bool,
) -> Vec<FeatureObservation> {
    let count = window.clamp(1, MAX_INPUTS_PER_PREDICTION);
    let mut observations = state
        .recent_window(MAX_INPUTS_PER_PREDICTION)
        .iter()
        .rev()
        .filter(|observation| predicate(observation))
        .take(count)
        .copied()
        .collect::<Vec<_>>();
    observations.reverse();
    observations
}

fn zscore_signal_value(observations: &[FeatureObservation], threshold: f64) -> i8 {
    if observations.len() < 2 {
        return 0;
    }

    let count = observations.len() as f64;
    let mean = observations
        .iter()
        .map(score_from_filtered_observation)
        .sum::<f64>()
        / count;
    let variance = observations
        .iter()
        .map(|observation| {
            let delta = score_from_filtered_observation(observation) - mean;
            delta * delta
        })
        .sum::<f64>()
        / count;
    let stddev = variance.sqrt();
    if stddev == 0.0 {
        return 0;
    }

    let latest = score_from_filtered_observation(
        observations
            .last()
            .expect("observation length is checked before z-score calculation"),
    );
    let zscore = (latest - mean) / stddev;
    if zscore >= threshold {
        1
    } else if zscore <= -threshold {
        -1
    } else {
        0
    }
}

fn score_from_filtered_observation(observation: &FeatureObservation) -> f64 {
    observation
        .score
        .expect("score observations are filtered before z-score calculation")
}

fn empty_snapshot() -> SymbolSnapshot {
    SymbolSnapshot {
        signal_value: 0,
        input_event_ids_used: InputSet::empty(),
        max_input_received_time: 0,
        max_input_sequence: 0,
        max_input_event_key: None,
        feature_recipe_hash: None,
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

    #[test]
    fn score_window_snapshot_buckets_latest_zscore() {
        let mut store = StateStore::new();
        let events = [
            Event::new("px1", 10, 10, 1, EventRole::Feature, "XYZ", "score=10"),
            Event::new("px2", 20, 20, 2, EventRole::Feature, "XYZ", "score=10"),
            Event::new("px3", 30, 30, 3, EventRole::Feature, "XYZ", "score=10"),
            Event::new("px4", 40, 40, 4, EventRole::Feature, "XYZ", "score=30"),
        ];

        for event in &events {
            store.writer().apply(event).unwrap();
        }

        let snapshot = store
            .as_of_view()
            .score_window_snapshot(events[0].symbol_key, 5, 1.0);

        assert_eq!(snapshot.signal_value, 1);
        assert_eq!(snapshot.input_event_ids_used.len(), 4);
        assert_eq!(snapshot.max_input_event_key, Some(events[3].event_key));
    }

    #[test]
    fn sentiment_snapshot_ignores_score_only_features() {
        let mut store = StateStore::new();
        let events = [
            Event::new(
                "s1",
                10,
                10,
                1,
                EventRole::Feature,
                "XYZ",
                "sentiment=positive",
            ),
            Event::new("px1", 20, 20, 2, EventRole::Feature, "XYZ", "score=100"),
        ];

        for event in &events {
            store.writer().apply(event).unwrap();
        }

        let snapshot = store.as_of_view().snapshot(events[0].symbol_key);

        assert_eq!(snapshot.signal_value, 1);
        assert_eq!(
            snapshot.input_event_ids_used.single_key(),
            Some(events[0].event_key)
        );
    }
}
