use crate::log::fnv1a64;
use std::collections::BTreeMap;

/// Maximum number of recent feature observations and input event keys retained.
///
/// This cap applies both to per-symbol state history and to inline
/// `PredictionRecord` provenance. The trade-off is intentional: short-window
/// signals get bounded state and stack-backed provenance in the hot loop, while
/// long-window signals need a separate recipe/snapshot design instead of asking
/// this engine to remember or embed every input key.
pub const MAX_INPUTS_PER_PREDICTION: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// Stable compact event identifier used in provenance records.
///
/// Event keys are non-cryptographic FNV-1a 64-bit identifiers derived from
/// labels. Replay validates the catalog so collisions are rejected before state
/// updates; these keys are compact identities, not cryptographic commitments.
pub struct EventKey(pub u64);

impl EventKey {
    /// Derives an event key from a human-readable label.
    pub fn from_label(label: &str) -> Self {
        Self(fnv1a64(label.as_bytes()))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// Stable compact symbol identifier used by replay state and records.
///
/// Symbol IDs use the same non-cryptographic FNV-1a 64-bit derivation as
/// `EventKey`. The symbol catalog rejects label/id mismatches and collisions
/// before replay enters the hot loop.
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
/// keep using `SymbolId`; slots are for indexing replay state. This is the
/// cold-path/hot-path split: strings are accepted at ingestion, interned once,
/// and then replaced by dense integer slots before state updates begin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolSlot(usize);

impl SymbolSlot {
    pub(crate) fn new(index: usize) -> Self {
        Self(index)
    }

    /// Returns the replay-local dense index.
    pub(crate) fn as_usize(self) -> usize {
        self.0
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
    /// Keys beyond `MAX_INPUTS_PER_PREDICTION` are silently truncated by design.
    /// Built-in signals keep their windows at or below that cap. Custom signals
    /// that need larger provenance must provide a separate recipe/snapshot
    /// commitment instead of relying on this inline set.
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
}
