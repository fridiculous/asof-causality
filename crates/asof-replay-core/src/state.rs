use crate::{Event, InputSet, Sentiment};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolSnapshot {
    pub signal_value: i8,
    pub input_event_ids_used: InputSet,
    pub max_input_received_time: u64,
    pub max_input_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SymbolState {
    sentiment: Sentiment,
    source_input: InputSet,
    source_received_time: u64,
    source_sequence: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct StateStore {
    by_symbol: BTreeMap<String, SymbolState>,
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

        self.store.by_symbol.insert(
            event.symbol.clone(),
            SymbolState {
                sentiment,
                source_input: InputSet::one(event.event_key),
                source_received_time: event.received_time,
                source_sequence: event.sequence,
            },
        );

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AsOfView<'a> {
    store: &'a StateStore,
}

impl AsOfView<'_> {
    pub fn snapshot(&self, symbol: &str) -> SymbolSnapshot {
        match self.store.by_symbol.get(symbol) {
            Some(state) => SymbolSnapshot {
                signal_value: state.sentiment.signal_value(),
                input_event_ids_used: state.source_input,
                max_input_received_time: state
                    .source_input
                    .max_received_time(state.source_received_time),
                max_input_sequence: state.source_input.max_sequence(state.source_sequence),
            },
            None => SymbolSnapshot {
                signal_value: 0,
                input_event_ids_used: InputSet::empty(),
                max_input_received_time: 0,
                max_input_sequence: 0,
            },
        }
    }
}
