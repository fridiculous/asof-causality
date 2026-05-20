use crate::{Event, EventKey, InputSet};
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictionRecord {
    pub prediction_event_key: EventKey,
    pub prediction_time: u64,
    pub prediction_sequence: u64,
    pub symbol: String,
    pub signal_value: i8,
    pub input_event_ids_used: InputSet,
    pub max_input_received_time: u64,
    pub max_input_sequence: u64,
    pub max_input_event_key: Option<EventKey>,
}

impl PredictionRecord {
    pub fn canonical_line(&self, event_labels: &BTreeMap<EventKey, String>) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            format_replay_key(
                self.prediction_time,
                self.prediction_sequence,
                self.prediction_event_key,
                event_labels
            ),
            self.symbol,
            self.signal_value,
            self.input_event_ids_used.format_with(event_labels),
            self.max_input_replay_key(event_labels)
        )
    }

    pub fn max_input_replay_key(&self, event_labels: &BTreeMap<EventKey, String>) -> String {
        self.max_input_event_key
            .map(|event_key| {
                format_replay_key(
                    self.max_input_received_time,
                    self.max_input_sequence,
                    event_key,
                    event_labels,
                )
            })
            .unwrap_or_else(|| "-".to_string())
    }

    pub fn violates_replay_key_order(&self, event_labels: &BTreeMap<EventKey, String>) -> bool {
        let Some(max_input_event_key) = self.max_input_event_key else {
            return false;
        };
        let max_input_event_id = label_for(max_input_event_key, event_labels);
        let prediction_event_id = label_for(self.prediction_event_key, event_labels);

        (
            self.max_input_received_time,
            self.max_input_sequence,
            max_input_event_id.as_str(),
        ) > (
            self.prediction_time,
            self.prediction_sequence,
            prediction_event_id.as_str(),
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

    pub fn impossible_predictions(&self) -> Vec<&PredictionRecord> {
        self.records
            .iter()
            .filter(|record| record.violates_replay_key_order(&self.event_labels))
            .collect()
    }
}

fn format_replay_key(
    received_time: u64,
    sequence: u64,
    event_key: EventKey,
    event_labels: &BTreeMap<EventKey, String>,
) -> String {
    format!(
        "{}:{}:{}",
        received_time,
        sequence,
        label_for(event_key, event_labels)
    )
}

fn label_for(event_key: EventKey, event_labels: &BTreeMap<EventKey, String>) -> String {
    event_labels
        .get(&event_key)
        .cloned()
        .unwrap_or_else(|| format!("event_key:{:016x}", event_key.0))
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
