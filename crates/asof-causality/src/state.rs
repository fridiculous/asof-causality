use crate::{
    Event, EventKey, FeatureRecipeHash, FixedDecimal, InputSet, Sentiment, SymbolSlot,
    FIXED_DECIMAL_SCALE, MAX_INPUTS_PER_PREDICTION,
};

#[derive(Debug, Clone, PartialEq)]
/// Signal-visible evaluation for one symbol at one replay point.
pub struct SignalEvaluation {
    /// Emitted signal value.
    pub signal_value: i8,
    /// Input event keys used to produce the value.
    pub input_event_ids_used: InputSet,
    /// Maximum received time among input events used by the prediction.
    pub max_input_received_time: u64,
    /// Maximum received sequence number among input events used by the prediction.
    pub max_input_received_sequence_number: u64,
    /// Event key for the maximum input replay key, when an input exists.
    pub max_input_event_key: Option<EventKey>,
    /// Optional precomputed feature recipe hash supplied by a signal.
    pub feature_recipe_hash: Option<FeatureRecipeHash>,
}

#[derive(Debug, Clone, PartialEq)]
struct SymbolState {
    // Fixed-size recent state keeps signal evaluation allocation-free. The cap
    // is real state history, not only output provenance: the 9th observation
    // evicts the oldest one. This is deliberately a short-window engine until
    // larger recipe/snapshot commitments replace bounded in-memory history.
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
    score: Option<FixedDecimal>,
    input_key: EventKey,
    received_time: u64,
    received_sequence_number: u64,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct StateStore {
    // Hot-loop state is indexed by replay-local `SymbolSlot`, never by symbol
    // strings. The symbol catalog pays the parsing/interning cost before replay
    // starts; apply/evaluate then reduce to dense Vec access.
    by_symbol_slot: Vec<SymbolState>,
}

impl StateStore {
    pub(crate) fn with_symbol_count(symbols: usize) -> Self {
        Self {
            by_symbol_slot: vec![SymbolState::default(); symbols],
        }
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
    pub(crate) fn apply(
        &mut self,
        event: &Event,
        symbol: SymbolSlot,
    ) -> Result<(), crate::ParseEventError> {
        let Some(values) = event.feature_values()? else {
            return Ok(());
        };

        self.store.by_symbol_slot[symbol.as_usize()].push(FeatureObservation {
            sentiment: values.sentiment,
            score: values.score,
            input_key: event.event_key,
            received_time: event.received_time,
            received_sequence_number: event.received_sequence_number,
        });

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
/// Opaque read-only state view exposed to signal implementations.
///
/// This is the correctness cage. Signal code can query only state that replay
/// has already applied for the current symbol slot; it cannot scan the full
/// event list, mutate state, or reach future rows through the type surface. The
/// view itself does not apply a timestamp filter; replay order decides which
/// events have been applied before constructing this view.
pub struct AsOfView<'a> {
    store: &'a StateStore,
}

impl AsOfView<'_> {
    /// Returns the latest received sentiment snapshot for `symbol`.
    pub fn snapshot(&self, symbol: SymbolSlot) -> SignalEvaluation {
        let state = &self.store.by_symbol_slot[symbol.as_usize()];
        match state
            .recent_window(MAX_INPUTS_PER_PREDICTION)
            .iter()
            .rev()
            .find(|observation| observation.sentiment.is_some())
            .copied()
        {
            Some(observation) => SignalEvaluation {
                signal_value: observation.sentiment.unwrap().signal_value(),
                input_event_ids_used: InputSet::one(observation.input_key),
                max_input_received_time: observation.received_time,
                max_input_received_sequence_number: observation.received_sequence_number,
                max_input_event_key: Some(observation.input_key),
                feature_recipe_hash: None,
            },
            None => empty_snapshot(),
        }
    }

    /// Returns a bounded recent sentiment-window snapshot for `symbol`.
    pub fn windowed_snapshot(&self, symbol: SymbolSlot, window: usize) -> SignalEvaluation {
        let state = &self.store.by_symbol_slot[symbol.as_usize()];

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
                observation.received_sequence_number,
                observation.input_key,
            ) > (
                max_observation.received_time,
                max_observation.received_sequence_number,
                max_observation.input_key,
            ) {
                max_observation = *observation;
            }
        }

        SignalEvaluation {
            signal_value: signal_sum.signum() as i8,
            input_event_ids_used: InputSet::from_ordered_keys(&keys[..observations.len()]),
            max_input_received_time: max_observation.received_time,
            max_input_received_sequence_number: max_observation.received_sequence_number,
            max_input_event_key: Some(max_observation.input_key),
            feature_recipe_hash: None,
        }
    }

    /// Returns a bounded recent numeric-score z-score snapshot for `symbol`.
    pub fn score_window_snapshot(
        &self,
        symbol: SymbolSlot,
        window: usize,
        threshold_scaled: i64,
    ) -> SignalEvaluation {
        let state = &self.store.by_symbol_slot[symbol.as_usize()];

        let observations = recent_observations_with_score(state, window);
        if observations.is_empty() {
            return empty_snapshot();
        }

        score_snapshot_from_observations(
            &observations,
            zscore_signal_value(&observations, threshold_scaled),
        )
    }

    /// Returns a fixed-point volatility-adjusted momentum signal evaluation.
    pub fn score_momentum_snapshot(
        &self,
        symbol: SymbolSlot,
        fast_window: usize,
        slow_window: usize,
        min_trend: FixedDecimal,
        volatility_divisor: i64,
    ) -> SignalEvaluation {
        let state = &self.store.by_symbol_slot[symbol.as_usize()];

        let slow_window = slow_window.clamp(1, MAX_INPUTS_PER_PREDICTION);
        let fast_window = fast_window.clamp(1, slow_window);
        let observations = recent_observations_with_score(state, slow_window);
        if observations.is_empty() {
            return empty_snapshot();
        }
        if observations.len() < slow_window {
            return score_snapshot_from_observations(&observations, 0);
        }

        let slow_mean = mean_score(&observations);
        let fast_mean = mean_score(&observations[observations.len() - fast_window..]);
        let trend = i128::from(fast_mean.scaled()) - i128::from(slow_mean.scaled());
        let volatility = i128::from(mean_absolute_deviation(&observations, slow_mean).scaled());
        let volatility_gate = volatility / i128::from(volatility_divisor.max(1));
        let threshold = i128::from(min_trend.scaled()).abs().max(volatility_gate);
        let signal_value = if trend > 0 && trend >= threshold {
            1
        } else if trend < 0 && trend <= -threshold {
            -1
        } else {
            0
        };

        score_snapshot_from_observations(&observations, signal_value)
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

fn zscore_signal_value(observations: &[FeatureObservation], threshold_scaled: i64) -> i8 {
    if observations.len() < 2 {
        return 0;
    }

    let count = observations.len() as i128;
    let sum = observations
        .iter()
        .map(|observation| i128::from(score_from_filtered_observation(observation).scaled()))
        .sum::<i128>();
    let mut sum_squared_deviations = 0_i128;
    for observation in observations {
        let Some(delta) = i128::from(score_from_filtered_observation(observation).scaled())
            .checked_mul(count)
            .and_then(|value| value.checked_sub(sum))
        else {
            return 0;
        };
        let Some(squared_delta) = delta.checked_mul(delta) else {
            return 0;
        };
        let Some(next_sum) = sum_squared_deviations.checked_add(squared_delta) else {
            return 0;
        };
        sum_squared_deviations = next_sum;
    }
    if sum_squared_deviations == 0 {
        return 0;
    }

    let Some(latest_delta) = i128::from(
        score_from_filtered_observation(
            observations
                .last()
                .expect("observation length is checked before z-score calculation"),
        )
        .scaled(),
    )
    .checked_mul(count)
    .and_then(|value| value.checked_sub(sum)) else {
        return 0;
    };
    if latest_delta == 0 {
        return 0;
    }

    let threshold = i128::from(threshold_scaled).abs();
    let scale = i128::from(FIXED_DECIMAL_SCALE);
    let Some(lhs) = latest_delta
        .checked_mul(latest_delta)
        .and_then(|value| value.checked_mul(count))
        .and_then(|value| value.checked_mul(scale))
        .and_then(|value| value.checked_mul(scale))
    else {
        return 0;
    };
    let Some(rhs) = threshold
        .checked_mul(threshold)
        .and_then(|value| value.checked_mul(sum_squared_deviations))
    else {
        return 0;
    };

    if lhs < rhs {
        return 0;
    }

    if latest_delta > 0 {
        1
    } else {
        -1
    }
}

fn score_from_filtered_observation(observation: &FeatureObservation) -> FixedDecimal {
    observation
        .score
        .expect("score observations are filtered before numeric calculations")
}

fn mean_score(observations: &[FeatureObservation]) -> FixedDecimal {
    let sum = observations
        .iter()
        .map(|observation| i128::from(score_from_filtered_observation(observation).scaled()))
        .sum::<i128>();
    fixed_decimal_from_i128(sum / observations.len() as i128)
}

fn mean_absolute_deviation(
    observations: &[FeatureObservation],
    center: FixedDecimal,
) -> FixedDecimal {
    let sum = observations
        .iter()
        .map(|observation| {
            (i128::from(score_from_filtered_observation(observation).scaled())
                - i128::from(center.scaled()))
            .abs()
        })
        .sum::<i128>();
    fixed_decimal_from_i128(sum / observations.len() as i128)
}

fn fixed_decimal_from_i128(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64)
}

fn score_snapshot_from_observations(
    observations: &[FeatureObservation],
    signal_value: i8,
) -> SignalEvaluation {
    let mut keys = [EventKey::default(); MAX_INPUTS_PER_PREDICTION];
    let mut max_observation = observations[0];

    for (index, observation) in observations.iter().enumerate() {
        keys[index] = observation.input_key;
        if (
            observation.received_time,
            observation.received_sequence_number,
            observation.input_key,
        ) > (
            max_observation.received_time,
            max_observation.received_sequence_number,
            max_observation.input_key,
        ) {
            max_observation = *observation;
        }
    }

    SignalEvaluation {
        signal_value,
        input_event_ids_used: InputSet::from_ordered_keys(&keys[..observations.len()]),
        max_input_received_time: max_observation.received_time,
        max_input_received_sequence_number: max_observation.received_sequence_number,
        max_input_event_key: Some(max_observation.input_key),
        feature_recipe_hash: None,
    }
}

fn empty_snapshot() -> SignalEvaluation {
    SignalEvaluation {
        signal_value: 0,
        input_event_ids_used: InputSet::empty(),
        max_input_received_time: 0,
        max_input_received_sequence_number: 0,
        max_input_event_key: None,
        feature_recipe_hash: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::SymbolCatalog;
    use crate::EventRole;

    fn store_with_events(events: &[Event]) -> (StateStore, SymbolCatalog) {
        let catalog = SymbolCatalog::from_events(events).unwrap();
        let mut store = StateStore::with_symbol_count(catalog.len());
        for event in events {
            let symbol = catalog.slot_for_event(event).unwrap();
            store.writer().apply(event, symbol).unwrap();
        }
        (store, catalog)
    }

    #[test]
    fn windowed_snapshot_records_multiple_feature_inputs() {
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
        let (store, catalog) = store_with_events(&events);
        let symbol = catalog.slot_for_event(&events[0]).unwrap();

        let snapshot = store.as_of_view().windowed_snapshot(symbol, 3);

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
        assert_eq!(snapshot.max_input_received_sequence_number, 3);
    }

    #[test]
    fn score_window_snapshot_buckets_latest_zscore() {
        let events = [
            Event::new("px1", 10, 10, 1, EventRole::Feature, "XYZ", "score=10"),
            Event::new("px2", 20, 20, 2, EventRole::Feature, "XYZ", "score=10"),
            Event::new("px3", 30, 30, 3, EventRole::Feature, "XYZ", "score=10"),
            Event::new("px4", 40, 40, 4, EventRole::Feature, "XYZ", "score=30"),
        ];
        let (store, catalog) = store_with_events(&events);
        let symbol = catalog.slot_for_event(&events[0]).unwrap();

        let snapshot = store
            .as_of_view()
            .score_window_snapshot(symbol, 5, FIXED_DECIMAL_SCALE);

        assert_eq!(snapshot.signal_value, 1);
        assert_eq!(snapshot.input_event_ids_used.len(), 4);
        assert_eq!(snapshot.max_input_event_key, Some(events[3].event_key));
    }

    #[test]
    fn score_window_snapshot_returns_zero_when_checked_zscore_comparison_overflows() {
        let events = [
            Event::new(
                "px1",
                10,
                10,
                1,
                EventRole::Feature,
                "XYZ",
                "score=1000000000000",
            ),
            Event::new("px2", 20, 20, 2, EventRole::Feature, "XYZ", "score=0"),
            Event::new("px3", 30, 30, 3, EventRole::Feature, "XYZ", "score=0"),
            Event::new("px4", 40, 40, 4, EventRole::Feature, "XYZ", "score=0"),
            Event::new("px5", 50, 50, 5, EventRole::Feature, "XYZ", "score=0"),
        ];
        let (store, catalog) = store_with_events(&events);
        let symbol = catalog.slot_for_event(&events[0]).unwrap();

        let snapshot = store
            .as_of_view()
            .score_window_snapshot(symbol, 5, FIXED_DECIMAL_SCALE);

        assert_eq!(snapshot.signal_value, 0);
        assert_eq!(snapshot.input_event_ids_used.len(), 5);
        assert_eq!(snapshot.max_input_event_key, Some(events[4].event_key));
    }

    #[test]
    fn score_window_snapshot_returns_zero_when_squared_deviation_overflows() {
        let events = [
            Event::new("px1", 10, 10, 1, EventRole::Feature, "XYZ", "score=0"),
            Event::new("px2", 20, 20, 2, EventRole::Feature, "XYZ", "score=0"),
            Event::new("px3", 30, 30, 3, EventRole::Feature, "XYZ", "score=0"),
            Event::new("px4", 40, 40, 4, EventRole::Feature, "XYZ", "score=0"),
            Event::new("px5", 50, 50, 5, EventRole::Feature, "XYZ", "score=0"),
            Event::new("px6", 60, 60, 6, EventRole::Feature, "XYZ", "score=0"),
            Event::new("px7", 70, 70, 7, EventRole::Feature, "XYZ", "score=0"),
            Event::new(
                "px8",
                80,
                80,
                8,
                EventRole::Feature,
                "XYZ",
                "score=4000000000000",
            ),
        ];
        let (store, catalog) = store_with_events(&events);
        let symbol = catalog.slot_for_event(&events[0]).unwrap();

        let snapshot = store
            .as_of_view()
            .score_window_snapshot(symbol, 8, FIXED_DECIMAL_SCALE);

        assert_eq!(snapshot.signal_value, 0);
        assert_eq!(snapshot.input_event_ids_used.len(), 8);
        assert_eq!(snapshot.max_input_event_key, Some(events[7].event_key));
    }

    #[test]
    fn score_momentum_snapshot_uses_fixed_point_crossover() {
        let events = [
            Event::new("px1", 10, 10, 1, EventRole::Feature, "XYZ", "score=10"),
            Event::new("px2", 20, 20, 2, EventRole::Feature, "XYZ", "score=10"),
            Event::new("px3", 30, 30, 3, EventRole::Feature, "XYZ", "score=10"),
            Event::new("px4", 40, 40, 4, EventRole::Feature, "XYZ", "score=30"),
        ];
        let (store, catalog) = store_with_events(&events);
        let symbol = catalog.slot_for_event(&events[0]).unwrap();

        let snapshot = store.as_of_view().score_momentum_snapshot(
            symbol,
            2,
            4,
            FixedDecimal::from_scaled(0),
            2,
        );

        assert_eq!(snapshot.signal_value, 1);
        assert_eq!(snapshot.input_event_ids_used.len(), 4);
        assert_eq!(snapshot.max_input_event_key, Some(events[3].event_key));
    }

    #[test]
    fn sentiment_snapshot_ignores_score_only_features() {
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
        let (store, catalog) = store_with_events(&events);
        let symbol = catalog.slot_for_event(&events[0]).unwrap();

        let snapshot = store.as_of_view().snapshot(symbol);

        assert_eq!(snapshot.signal_value, 1);
        assert_eq!(
            snapshot.input_event_ids_used.single_key(),
            Some(events[0].event_key)
        );
    }
}
