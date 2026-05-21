use crate::event::{Event, ParseEventError};
use crate::log::fnv1a64;
use std::collections::BTreeMap;

/// Maximum number of input event keys stored inline in a prediction record.
///
/// This is deliberately fixed-capacity so the replay path can carry multi-input
/// provenance without allocating a `Vec` per prediction.
pub const MAX_INPUTS_PER_PREDICTION: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// Stable compact event identifier used in provenance records.
pub struct EventKey(pub u64);

impl EventKey {
    /// Derives an event key from a human-readable label.
    pub fn from_label(label: &str) -> Self {
        Self(fnv1a64(label.as_bytes()))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// Stable compact symbol identifier used by replay state and records.
pub struct SymbolId(pub u64);

impl SymbolId {
    /// Derives a symbol identifier from a human-readable symbol label.
    pub fn from_label(label: &str) -> Self {
        Self(fnv1a64(label.as_bytes()))
    }
}

/// Dense, replay-local symbol index.
///
/// A `SymbolSlot` is stable only within the symbol catalog built for one replay.
/// Use it only with state that was sized from the same catalog. Audit records
/// keep using `SymbolId`; slots are for indexing replay state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolSlot(usize);

impl SymbolSlot {
    pub(crate) fn new(index: usize) -> Self {
        Self(index)
    }

    /// Returns the replay-local dense index.
    pub fn as_usize(self) -> usize {
        self.0
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct SymbolCatalog {
    slots_by_label: BTreeMap<String, SymbolSlot>,
    slots_by_id: BTreeMap<SymbolId, SymbolSlot>,
    labels_by_slot: Vec<String>,
    ids_by_slot: Vec<SymbolId>,
}

impl SymbolCatalog {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn from_events(events: &[Event]) -> Result<Self, ParseEventError> {
        let mut catalog = Self::new();
        for event in events {
            catalog.intern_event(event)?;
        }
        Ok(catalog)
    }

    pub(crate) fn len(&self) -> usize {
        self.labels_by_slot.len()
    }

    pub(crate) fn symbol_labels(&self) -> BTreeMap<SymbolId, String> {
        self.ids_by_slot
            .iter()
            .copied()
            .zip(self.labels_by_slot.iter().cloned())
            .collect()
    }

    pub(crate) fn intern_event(&mut self, event: &Event) -> Result<SymbolSlot, ParseEventError> {
        self.intern(&event.symbol, event.symbol_key)
    }

    #[cfg(test)]
    pub(crate) fn slot_for_event(&self, event: &Event) -> Result<SymbolSlot, ParseEventError> {
        let Some(slot) = self.slots_by_label.get(&event.symbol).copied() else {
            return Err(ParseEventError::UnknownSymbol {
                symbol: event.symbol.clone(),
                symbol_id: event.symbol_key,
            });
        };

        let expected = self.ids_by_slot[slot.as_usize()];
        if expected != event.symbol_key {
            return Err(ParseEventError::SymbolIdentityMismatch {
                symbol: event.symbol.clone(),
                expected,
                actual: event.symbol_key,
            });
        }

        Ok(slot)
    }

    fn intern(&mut self, label: &str, symbol_id: SymbolId) -> Result<SymbolSlot, ParseEventError> {
        if let Some(slot) = self.slots_by_label.get(label).copied() {
            let expected = self.ids_by_slot[slot.as_usize()];
            if expected != symbol_id {
                return Err(ParseEventError::SymbolIdentityMismatch {
                    symbol: label.to_string(),
                    expected,
                    actual: symbol_id,
                });
            }
            return Ok(slot);
        }

        if let Some(slot) = self.slots_by_id.get(&symbol_id).copied() {
            return Err(ParseEventError::SymbolIdCollision {
                symbol_id,
                existing_symbol: self.labels_by_slot[slot.as_usize()].clone(),
                conflicting_symbol: label.to_string(),
            });
        }

        let slot = SymbolSlot::new(self.labels_by_slot.len());
        self.slots_by_label.insert(label.to_string(), slot);
        self.slots_by_id.insert(symbol_id, slot);
        self.labels_by_slot.push(label.to_string());
        self.ids_by_slot.push(symbol_id);
        Ok(slot)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Fixed-capacity set of input event keys used by a prediction.
pub enum InputSet {
    /// No input event was used.
    Empty,
    /// A single input event was used.
    One(EventKey),
    /// Multiple ordered input events were used.
    Many {
        /// Fixed-capacity event-key storage.
        keys: [EventKey; MAX_INPUTS_PER_PREDICTION],
        /// Number of populated keys.
        len: u8,
    },
}

impl InputSet {
    /// Returns an empty input set.
    pub fn empty() -> Self {
        Self::Empty
    }

    /// Returns an input set containing one key.
    pub fn one(event_key: EventKey) -> Self {
        Self::One(event_key)
    }

    /// Builds a deterministic, deduplicated input set.
    ///
    /// Keys beyond `MAX_INPUTS_PER_PREDICTION` are truncated by design. Built-in
    /// signals keep their windows at or below that cap.
    pub fn from_ordered_keys(keys: &[EventKey]) -> Self {
        let mut unique = [EventKey::default(); MAX_INPUTS_PER_PREDICTION];
        let mut len = 0;

        for key in keys.iter().copied() {
            if unique[..len].contains(&key) {
                continue;
            }
            if len == MAX_INPUTS_PER_PREDICTION {
                break;
            }
            unique[len] = key;
            len += 1;
        }

        match len {
            0 => Self::Empty,
            1 => Self::One(unique[0]),
            _ => Self::Many {
                keys: unique,
                len: len as u8,
            },
        }
    }

    /// Returns the number of input keys in the set.
    pub fn len(self) -> usize {
        match self {
            Self::Empty => 0,
            Self::One(_) => 1,
            Self::Many { len, .. } => usize::from(len),
        }
    }

    /// Returns whether the set contains no input keys.
    pub fn is_empty(self) -> bool {
        matches!(self, Self::Empty)
    }

    /// Iterates over input keys in deterministic order.
    pub fn iter(self) -> InputSetIter {
        let mut keys = [EventKey::default(); MAX_INPUTS_PER_PREDICTION];
        let len = match self {
            Self::Empty => 0,
            Self::One(key) => {
                keys[0] = key;
                1
            }
            Self::Many {
                keys: stored_keys,
                len,
            } => {
                keys = stored_keys;
                usize::from(len)
            }
        };

        InputSetIter {
            keys,
            len,
            index: 0,
        }
    }

    /// Returns whether the set contains `event_key`.
    pub fn contains_key(self, event_key: EventKey) -> bool {
        self.iter().any(|key| key == event_key)
    }

    /// Returns the one key if the set contains exactly one input.
    pub fn single_key(self) -> Option<EventKey> {
        match self {
            Self::Empty => None,
            Self::One(key) => Some(key),
            Self::Many { len: 1, keys } => Some(keys[0]),
            Self::Many { .. } => None,
        }
    }

    /// Returns the source received time when at least one input was used.
    pub fn max_received_time(self, received_time: u64) -> u64 {
        match self {
            Self::Empty => 0,
            Self::One(_) | Self::Many { .. } => received_time,
        }
    }

    /// Formats event keys with human labels when available.
    pub fn format_with(self, labels: &BTreeMap<EventKey, String>) -> String {
        match self {
            Self::Empty => "-".to_string(),
            Self::One(key) => format_key(key, labels),
            Self::Many { .. } => self
                .iter()
                .map(|key| format_key(key, labels))
                .collect::<Vec<_>>()
                .join(","),
        }
    }
}

#[derive(Debug, Clone, Copy)]
/// Iterator over a fixed-capacity [`InputSet`].
pub struct InputSetIter {
    keys: [EventKey; MAX_INPUTS_PER_PREDICTION],
    len: usize,
    index: usize,
}

impl Iterator for InputSetIter {
    /// Input event key yielded by the iterator.
    type Item = EventKey;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.len {
            return None;
        }

        let key = self.keys[self.index];
        self.index += 1;
        Some(key)
    }
}

fn format_key(key: EventKey, labels: &BTreeMap<EventKey, String>) -> String {
    labels
        .get(&key)
        .cloned()
        .unwrap_or_else(|| format!("event_key:{:016x}", key.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_preserves_order_and_deduplicates() {
        let a = EventKey(1);
        let b = EventKey(2);
        let inputs = InputSet::from_ordered_keys(&[a, b, a]);

        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs.iter().collect::<Vec<_>>(), vec![a, b]);
        assert!(inputs.contains_key(a));
        assert!(inputs.contains_key(b));
        assert_eq!(inputs.single_key(), None);
    }

    #[test]
    fn many_is_fixed_capacity() {
        let keys = [
            EventKey(1),
            EventKey(2),
            EventKey(3),
            EventKey(4),
            EventKey(5),
            EventKey(6),
            EventKey(7),
            EventKey(8),
            EventKey(9),
        ];
        let inputs = InputSet::from_ordered_keys(&keys);

        assert_eq!(inputs.len(), MAX_INPUTS_PER_PREDICTION);
        assert!(inputs.contains_key(EventKey(8)));
        assert!(!inputs.contains_key(EventKey(9)));
    }

    #[test]
    fn formats_many_inputs_with_labels() {
        let a = EventKey(1);
        let b = EventKey(2);
        let mut labels = BTreeMap::new();
        labels.insert(a, "first".to_string());
        labels.insert(b, "second".to_string());

        assert_eq!(
            InputSet::from_ordered_keys(&[a, b]).format_with(&labels),
            "first,second"
        );
    }

    #[test]
    fn symbol_catalog_assigns_dense_slots_by_first_seen_symbol() {
        let events = [
            Event::new(
                "a1",
                1,
                1,
                1,
                crate::EventRole::Feature,
                "AAPL",
                "sentiment=positive",
            ),
            Event::new(
                "m1",
                2,
                2,
                2,
                crate::EventRole::Feature,
                "MSFT",
                "sentiment=negative",
            ),
            Event::new("a2", 3, 3, 3, crate::EventRole::Prediction, "AAPL", ""),
        ];

        let catalog = SymbolCatalog::from_events(&events).unwrap();

        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog.slot_for_event(&events[0]).unwrap().as_usize(), 0);
        assert_eq!(catalog.slot_for_event(&events[1]).unwrap().as_usize(), 1);
        assert_eq!(catalog.slot_for_event(&events[2]).unwrap().as_usize(), 0);
    }

    #[test]
    fn symbol_catalog_rejects_id_collisions_between_labels() {
        let mut events = [
            Event::new(
                "a1",
                1,
                1,
                1,
                crate::EventRole::Feature,
                "AAPL",
                "sentiment=positive",
            ),
            Event::new(
                "m1",
                2,
                2,
                2,
                crate::EventRole::Feature,
                "MSFT",
                "sentiment=negative",
            ),
        ];
        events[1].symbol_key = events[0].symbol_key;

        let error = SymbolCatalog::from_events(&events).unwrap_err();

        assert!(matches!(
            error,
            ParseEventError::SymbolIdCollision {
                existing_symbol,
                conflicting_symbol,
                ..
            } if existing_symbol == "AAPL" && conflicting_symbol == "MSFT"
        ));
    }

    #[test]
    fn symbol_catalog_rejects_label_identity_changes() {
        let mut events = [
            Event::new(
                "a1",
                1,
                1,
                1,
                crate::EventRole::Feature,
                "AAPL",
                "sentiment=positive",
            ),
            Event::new(
                "a2",
                2,
                2,
                2,
                crate::EventRole::Feature,
                "AAPL",
                "sentiment=negative",
            ),
        ];
        events[1].symbol_key = SymbolId(42);

        let error = SymbolCatalog::from_events(&events).unwrap_err();

        assert!(matches!(
            error,
            ParseEventError::SymbolIdentityMismatch { symbol, .. } if symbol == "AAPL"
        ));
    }
}
