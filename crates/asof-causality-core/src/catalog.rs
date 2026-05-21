use crate::{Event, ParseEventError, SymbolId, SymbolSlot};
use std::collections::BTreeMap;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventRole, SymbolId};

    #[test]
    fn symbol_catalog_assigns_dense_slots_by_first_seen_symbol() {
        let events = [
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
            Event::new("a2", 3, 3, 3, EventRole::Prediction, "AAPL", ""),
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
                EventRole::Feature,
                "AAPL",
                "sentiment=positive",
            ),
            Event::new(
                "a2",
                2,
                2,
                2,
                EventRole::Feature,
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
