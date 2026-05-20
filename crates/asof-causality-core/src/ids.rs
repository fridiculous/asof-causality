use crate::log::fnv1a64;
use std::collections::BTreeMap;

/// Maximum number of input event keys stored inline in a prediction record.
///
/// This is deliberately fixed-capacity so the replay path can carry multi-input
/// provenance without allocating a `Vec` per prediction.
pub const MAX_INPUTS_PER_PREDICTION: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventKey(pub u64);

impl EventKey {
    pub fn from_label(label: &str) -> Self {
        Self(fnv1a64(label.as_bytes()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSet {
    Empty,
    One(EventKey),
    Many {
        keys: [EventKey; MAX_INPUTS_PER_PREDICTION],
        len: u8,
    },
}

impl InputSet {
    pub fn empty() -> Self {
        Self::Empty
    }

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

    pub fn len(self) -> usize {
        match self {
            Self::Empty => 0,
            Self::One(_) => 1,
            Self::Many { len, .. } => usize::from(len),
        }
    }

    pub fn is_empty(self) -> bool {
        matches!(self, Self::Empty)
    }

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

    pub fn contains_key(self, event_key: EventKey) -> bool {
        self.iter().any(|key| key == event_key)
    }

    pub fn single_key(self) -> Option<EventKey> {
        match self {
            Self::Empty => None,
            Self::One(key) => Some(key),
            Self::Many { len: 1, keys } => Some(keys[0]),
            Self::Many { .. } => None,
        }
    }

    pub fn max_received_time(self, received_time: u64) -> u64 {
        match self {
            Self::Empty => 0,
            Self::One(_) | Self::Many { .. } => received_time,
        }
    }

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
pub struct InputSetIter {
    keys: [EventKey; MAX_INPUTS_PER_PREDICTION],
    len: usize,
    index: usize,
}

impl Iterator for InputSetIter {
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
