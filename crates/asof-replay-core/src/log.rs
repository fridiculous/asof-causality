use crate::{Event, EventKey, InputSet};
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictionRecord {
    pub prediction_time: u64,
    pub prediction_sequence: u64,
    pub symbol: String,
    pub signal_value: i8,
    pub input_event_ids_used: InputSet,
    pub max_input_received_time: u64,
    pub max_input_sequence: u64,
}

impl PredictionRecord {
    pub fn prediction_replay_key(&self) -> (u64, u64) {
        (self.prediction_time, self.prediction_sequence)
    }

    pub fn max_input_replay_key(&self) -> (u64, u64) {
        (self.max_input_received_time, self.max_input_sequence)
    }

    pub fn uses_future_input(&self) -> bool {
        self.max_input_replay_key() > self.prediction_replay_key()
    }

    pub fn canonical_line(&self, event_labels: &BTreeMap<EventKey, String>) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            self.prediction_time,
            self.prediction_sequence,
            self.symbol,
            self.signal_value,
            self.input_event_ids_used.format_with(event_labels),
            self.max_input_received_time,
            self.max_input_sequence
        )
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PredictionLog {
    records: Vec<PredictionRecord>,
    event_labels: BTreeMap<EventKey, String>,
}

impl PredictionLog {
    pub fn with_event_catalog(events: &[Event]) -> Self {
        let event_labels = events
            .iter()
            .map(|event| (event.event_key, event.event_id.clone()))
            .collect();

        Self {
            records: Vec::new(),
            event_labels,
        }
    }

    pub fn append(&mut self, record: PredictionRecord) {
        self.records.push(record);
    }

    pub fn records(&self) -> &[PredictionRecord] {
        &self.records
    }

    pub fn transcript(&self) -> String {
        let mut output = String::new();
        for record in &self.records {
            let _ = writeln!(output, "{}", record.canonical_line(&self.event_labels));
        }
        output
    }

    pub fn transcript_hash(&self) -> u64 {
        fnv1a64(self.transcript().as_bytes())
    }
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}
