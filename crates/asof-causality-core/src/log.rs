use crate::catalog::SymbolCatalog;
use crate::{Event, EventKey, InputSet, ParseEventError, ReplayKey, SymbolId};
use std::collections::BTreeMap;
use std::fmt::Write;

/// Fixed 32-byte digest committing to a signal's feature recipe.
pub type FeatureRecipeHash = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq)]
/// Immutable prediction audit record emitted by replay.
pub struct PredictionRecord {
    /// Event key for the prediction event that caused this record.
    pub prediction_event_key: EventKey,
    /// Received time of the prediction event.
    pub prediction_time: u64,
    /// Received sequence number of the prediction event.
    pub prediction_received_sequence_number: u64,
    /// Symbol identifier for the prediction.
    pub symbol: SymbolId,
    /// Signal value emitted at the prediction replay key.
    pub signal_value: i8,
    /// Input events used to compute the prediction.
    pub input_event_ids_used: InputSet,
    /// Maximum input received time used by the prediction.
    pub max_input_received_time: u64,
    /// Maximum input received sequence number used by the prediction.
    pub max_input_received_sequence_number: u64,
    /// Event key for the maximum input replay key, when any input was used.
    pub max_input_event_key: Option<EventKey>,
    /// BLAKE3 recipe digest for the signal and input set.
    pub feature_recipe_hash: FeatureRecipeHash,
}

impl PredictionRecord {
    /// Formats a deterministic transcript line with human labels.
    pub fn canonical_line(
        &self,
        event_labels: &BTreeMap<EventKey, String>,
        symbol_labels: &BTreeMap<SymbolId, String>,
    ) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            format_replay_key(
                self.prediction_time,
                self.prediction_received_sequence_number,
                self.prediction_event_key,
                event_labels
            ),
            label_for_symbol(self.symbol, symbol_labels),
            self.signal_value,
            self.input_event_ids_used.format_with(event_labels),
            self.max_input_replay_key(event_labels)
        )
    }

    /// Formats the maximum input replay key for display.
    pub fn max_input_replay_key(&self, event_labels: &BTreeMap<EventKey, String>) -> String {
        self.max_input_event_key
            .map(|event_key| {
                format_replay_key(
                    self.max_input_received_time,
                    self.max_input_received_sequence_number,
                    event_key,
                    event_labels,
                )
            })
            .unwrap_or_else(|| "-".to_string())
    }

    /// Returns whether this record used an input after the prediction replay key.
    pub fn violates_replay_key_order(&self, event_labels: &BTreeMap<EventKey, String>) -> bool {
        let Some(max_input_event_key) = self.max_input_event_key else {
            return false;
        };
        let max_input_event_id = label_for(max_input_event_key, event_labels);
        let prediction_event_id = label_for(self.prediction_event_key, event_labels);

        ReplayKey::new(
            self.max_input_received_time,
            self.max_input_received_sequence_number,
            max_input_event_id.as_str(),
        ) > ReplayKey::new(
            self.prediction_time,
            self.prediction_received_sequence_number,
            prediction_event_id.as_str(),
        )
    }

    /// Returns the feature recipe hash as lowercase hex.
    pub fn feature_recipe_hash_hex(&self) -> String {
        hex_digest(&self.feature_recipe_hash)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
/// Append-only prediction log plus event and symbol catalogs.
pub struct PredictionLog {
    records: Vec<PredictionRecord>,
    event_labels: BTreeMap<EventKey, String>,
    symbol_labels: BTreeMap<SymbolId, String>,
}

impl PredictionLog {
    /// Creates an empty prediction log with labels collected from validated events.
    pub fn with_event_catalog(events: &[Event]) -> Result<Self, ParseEventError> {
        let symbol_catalog = SymbolCatalog::from_events(events)?;
        Ok(Self::with_symbol_catalog(events, &symbol_catalog))
    }

    pub(crate) fn with_symbol_catalog(events: &[Event], symbol_catalog: &SymbolCatalog) -> Self {
        Self {
            records: Vec::new(),
            event_labels: event_labels(events),
            symbol_labels: symbol_catalog.symbol_labels(),
        }
    }

    /// Appends one prediction record.
    pub fn append(&mut self, record: PredictionRecord) {
        self.records.push(record);
    }

    /// Returns all prediction records in append order.
    pub fn records(&self) -> &[PredictionRecord] {
        &self.records
    }

    /// Renders the deterministic text transcript.
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

    /// Returns the legacy 64-bit transcript checksum.
    pub fn transcript_hash(&self) -> u64 {
        fnv1a64(self.transcript().as_bytes())
    }

    /// Returns the BLAKE3 transcript digest as lowercase hex.
    pub fn transcript_digest(&self) -> String {
        blake3_hex(self.transcript().as_bytes())
    }

    /// Returns records that violate the causality replay-key invariant.
    pub fn impossible_predictions(&self) -> Vec<&PredictionRecord> {
        self.records
            .iter()
            .filter(|record| record.violates_replay_key_order(&self.event_labels))
            .collect()
    }

    /// Returns the human label for an event key when known.
    pub fn event_label(&self, event_key: EventKey) -> String {
        label_for(event_key, &self.event_labels)
    }

    /// Returns the human label for a symbol key when known.
    pub fn symbol_label(&self, symbol: SymbolId) -> String {
        label_for_symbol(symbol, &self.symbol_labels)
    }

    /// Formats a replay key using event labels when available.
    pub fn format_replay_key(
        &self,
        received_time: u64,
        received_sequence_number: u64,
        event_key: EventKey,
    ) -> String {
        format_replay_key(
            received_time,
            received_sequence_number,
            event_key,
            &self.event_labels,
        )
    }

    /// Returns human labels for all input events in `inputs`.
    pub fn input_event_labels(&self, inputs: InputSet) -> Vec<String> {
        inputs
            .iter()
            .map(|event_key| label_for(event_key, &self.event_labels))
            .collect()
    }

    /// Formats a record's maximum input replay key, if it used any input.
    pub fn max_input_replay_key_value(&self, record: &PredictionRecord) -> Option<String> {
        record.max_input_event_key.map(|event_key| {
            format_replay_key(
                record.max_input_received_time,
                record.max_input_received_sequence_number,
                event_key,
                &self.event_labels,
            )
        })
    }

    /// Returns whether a prediction record satisfies the causality invariant.
    pub fn record_is_causal(&self, record: &PredictionRecord) -> bool {
        !record.violates_replay_key_order(&self.event_labels)
    }
}

fn event_labels(events: &[Event]) -> BTreeMap<EventKey, String> {
    events
        .iter()
        .map(|event| (event.event_key, event.event_id.clone()))
        .collect()
}

fn format_replay_key(
    received_time: u64,
    received_sequence_number: u64,
    event_key: EventKey,
    event_labels: &BTreeMap<EventKey, String>,
) -> String {
    format!(
        "{}:{}:{}",
        received_time,
        received_sequence_number,
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

/// Computes a non-cryptographic FNV-1a 64-bit checksum.
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

/// Computes a BLAKE3 digest and formats it as lowercase hex.
pub fn blake3_hex(bytes: &[u8]) -> String {
    hex_digest(&blake3_digest(bytes))
}

/// Computes a raw BLAKE3 digest.
pub fn blake3_digest(bytes: &[u8]) -> FeatureRecipeHash {
    *blake3::hash(bytes).as_bytes()
}

/// Formats a 32-byte digest as lowercase hex.
pub fn hex_digest(digest: &FeatureRecipeHash) -> String {
    let mut text = String::with_capacity(64);
    for byte in digest {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// Computes the feature recipe hash for a signal configuration and inputs.
pub fn feature_recipe_hash(
    signal_name: &str,
    config_descriptor: &str,
    inputs: InputSet,
) -> FeatureRecipeHash {
    let mut hasher = blake3::Hasher::new();
    // Internal to the recipe-hash input; independent of audit JSON schema versions.
    hasher.update(b"schema_version");
    hasher.update(&1_u32.to_le_bytes());
    update_hash_field(&mut hasher, b"signal", signal_name.as_bytes());
    update_hash_field(&mut hasher, b"config", config_descriptor.as_bytes());
    for input_key in inputs.iter() {
        hasher.update(b"input_key");
        hasher.update(&input_key.0.to_le_bytes());
    }
    *hasher.finalize().as_bytes()
}

fn update_hash_field(hasher: &mut blake3::Hasher, name: &[u8], value: &[u8]) {
    hasher.update(name);
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventRole;

    #[test]
    fn event_catalog_rejects_symbol_id_collisions() {
        let mut events = [
            Event::new(
                "a1",
                1,
                1,
                1,
                EventRole::Feature,
                "AAPL",
                "sentiment=positive",
            ),
            Event::new(
                "m1",
                2,
                2,
                2,
                EventRole::Feature,
                "MSFT",
                "sentiment=negative",
            ),
        ];
        events[1].symbol_key = events[0].symbol_key;

        let error = PredictionLog::with_event_catalog(&events).unwrap_err();

        assert!(matches!(
            error,
            ParseEventError::SymbolIdCollision {
                existing_symbol,
                conflicting_symbol,
                ..
            } if existing_symbol == "AAPL" && conflicting_symbol == "MSFT"
        ));
    }
}
