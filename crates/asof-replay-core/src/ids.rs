use crate::log::fnv1a64;
use std::collections::BTreeMap;

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
}

impl InputSet {
    pub fn empty() -> Self {
        Self::Empty
    }

    pub fn one(event_key: EventKey) -> Self {
        Self::One(event_key)
    }

    pub fn contains_key(self, event_key: EventKey) -> bool {
        matches!(self, Self::One(key) if key == event_key)
    }

    pub fn single_key(self) -> Option<EventKey> {
        match self {
            Self::Empty => None,
            Self::One(key) => Some(key),
        }
    }

    pub fn max_received_time(self, received_time: u64) -> u64 {
        match self {
            Self::Empty => 0,
            Self::One(_) => received_time,
        }
    }

    pub fn max_sequence(self, sequence: u64) -> u64 {
        match self {
            Self::Empty => 0,
            Self::One(_) => sequence,
        }
    }

    pub fn format_with(self, labels: &BTreeMap<EventKey, String>) -> String {
        match self {
            Self::Empty => "-".to_string(),
            Self::One(key) => labels
                .get(&key)
                .cloned()
                .unwrap_or_else(|| format!("event_key:{:016x}", key.0)),
        }
    }
}
