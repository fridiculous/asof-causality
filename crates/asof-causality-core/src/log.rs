use crate::{Event, EventKey, InputSet, SymbolId};
use std::collections::BTreeMap;
use std::fmt::Write;

pub type FeatureRecipeHash = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictionRecord {
    pub prediction_event_key: EventKey,
    pub prediction_time: u64,
    pub prediction_sequence: u64,
    pub symbol: SymbolId,
    pub signal_value: i8,
    pub input_event_ids_used: InputSet,
    pub max_input_received_time: u64,
    pub max_input_sequence: u64,
    pub max_input_event_key: Option<EventKey>,
    pub feature_recipe_hash: FeatureRecipeHash,
}

impl PredictionRecord {
    pub fn canonical_line(
        &self,
        event_labels: &BTreeMap<EventKey, String>,
        symbol_labels: &BTreeMap<SymbolId, String>,
    ) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            format_replay_key(
                self.prediction_time,
                self.prediction_sequence,
                self.prediction_event_key,
                event_labels
            ),
            label_for_symbol(self.symbol, symbol_labels),
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

    pub fn feature_recipe_hash_hex(&self) -> String {
        hex_digest(&self.feature_recipe_hash)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PredictionLog {
    records: Vec<PredictionRecord>,
    event_labels: BTreeMap<EventKey, String>,
    symbol_labels: BTreeMap<SymbolId, String>,
}

impl PredictionLog {
    pub fn with_event_catalog(events: &[Event]) -> Self {
        let event_labels = events
            .iter()
            .map(|event| (event.event_key, event.event_id.clone()))
            .collect();
        let symbol_labels = events
            .iter()
            .map(|event| (event.symbol_key, event.symbol.clone()))
            .collect();

        Self {
            records: Vec::new(),
            event_labels,
            symbol_labels,
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
            let _ = writeln!(
                output,
                "{}",
                record.canonical_line(&self.event_labels, &self.symbol_labels)
            );
        }
        output
    }

    pub fn transcript_hash(&self) -> u64 {
        fnv1a64(self.transcript().as_bytes())
    }

    pub fn transcript_digest(&self) -> String {
        blake3_hex(self.transcript().as_bytes())
    }

    pub fn impossible_predictions(&self) -> Vec<&PredictionRecord> {
        self.records
            .iter()
            .filter(|record| record.violates_replay_key_order(&self.event_labels))
            .collect()
    }

    pub fn event_label(&self, event_key: EventKey) -> String {
        label_for(event_key, &self.event_labels)
    }

    pub fn symbol_label(&self, symbol: SymbolId) -> String {
        label_for_symbol(symbol, &self.symbol_labels)
    }

    pub fn format_replay_key(
        &self,
        received_time: u64,
        sequence: u64,
        event_key: EventKey,
    ) -> String {
        format_replay_key(received_time, sequence, event_key, &self.event_labels)
    }

    pub fn input_event_labels(&self, inputs: InputSet) -> Vec<String> {
        inputs
            .iter()
            .map(|event_key| label_for(event_key, &self.event_labels))
            .collect()
    }

    pub fn max_input_replay_key_value(&self, record: &PredictionRecord) -> Option<String> {
        record.max_input_event_key.map(|event_key| {
            format_replay_key(
                record.max_input_received_time,
                record.max_input_sequence,
                event_key,
                &self.event_labels,
            )
        })
    }

    pub fn record_is_causal(&self, record: &PredictionRecord) -> bool {
        !record.violates_replay_key_order(&self.event_labels)
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

fn label_for_symbol(symbol: SymbolId, symbol_labels: &BTreeMap<SymbolId, String>) -> String {
    symbol_labels
        .get(&symbol)
        .cloned()
        .unwrap_or_else(|| format!("symbol:{:016x}", symbol.0))
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

pub fn blake3_hex(bytes: &[u8]) -> String {
    hex_digest(&blake3_digest(bytes))
}

pub fn blake3_digest(bytes: &[u8]) -> FeatureRecipeHash {
    *blake3::hash(bytes).as_bytes()
}

pub fn hex_digest(digest: &FeatureRecipeHash) -> String {
    let mut text = String::with_capacity(64);
    for byte in digest {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

pub fn feature_recipe_hash(
    signal_name: &str,
    config_descriptor: &str,
    inputs: InputSet,
) -> FeatureRecipeHash {
    let mut recipe = String::new();
    let _ = writeln!(recipe, "schema_version=1");
    let _ = writeln!(recipe, "signal={signal_name}");
    let _ = writeln!(recipe, "config={config_descriptor}");
    for input_key in inputs.iter() {
        let _ = writeln!(recipe, "input_key:{:016x}", input_key.0);
    }
    blake3_digest(recipe.as_bytes())
}
