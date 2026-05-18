use crate::event::{EventKind, NormalizedEvent};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureUpdate {
    pub symbol: String,
    pub price_cents: i64,
    pub size: u64,
    pub previous_price_cents: Option<i64>,
    pub return_bps: Option<i64>,
    pub cumulative_size: u64,
}

#[derive(Debug, Default)]
pub struct FeatureEngine {
    last_price_by_symbol: BTreeMap<String, i64>,
    cumulative_size_by_symbol: BTreeMap<String, u64>,
}

impl FeatureEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process(
        &mut self,
        event: &NormalizedEvent,
    ) -> Result<Option<FeatureUpdate>, FeatureError> {
        if event.kind != EventKind::Tick {
            return Ok(None);
        }

        let (price_cents, size) = parse_tick_payload(&event.payload)?;
        let previous_price_cents = self
            .last_price_by_symbol
            .insert(event.symbol.clone(), price_cents);

        let cumulative_size = self
            .cumulative_size_by_symbol
            .entry(event.symbol.clone())
            .and_modify(|current| *current += size)
            .or_insert(size);

        let return_bps = previous_price_cents
            .filter(|previous| *previous != 0)
            .map(|previous| ((price_cents - previous) * 10_000) / previous);

        Ok(Some(FeatureUpdate {
            symbol: event.symbol.clone(),
            price_cents,
            size,
            previous_price_cents,
            return_bps,
            cumulative_size: *cumulative_size,
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureError {
    MissingField(&'static str),
    MalformedPair(String),
    InvalidNumber {
        field: &'static str,
        value: String,
    },
}

impl fmt::Display for FeatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "missing tick payload field: {field}"),
            Self::MalformedPair(pair) => write!(f, "malformed payload pair: {pair}"),
            Self::InvalidNumber { field, value } => {
                write!(f, "invalid numeric field {field}: {value}")
            }
        }
    }
}

impl Error for FeatureError {}

fn parse_tick_payload(payload: &str) -> Result<(i64, u64), FeatureError> {
    let mut price_cents = None;
    let mut size = None;

    for pair in payload.split(',') {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| FeatureError::MalformedPair(pair.to_string()))?;

        match key.trim() {
            "price_cents" => {
                price_cents =
                    Some(value.trim().parse().map_err(|_| FeatureError::InvalidNumber {
                        field: "price_cents",
                        value: value.trim().to_string(),
                    })?);
            }
            "size" => {
                size = Some(value.trim().parse().map_err(|_| FeatureError::InvalidNumber {
                    field: "size",
                    value: value.trim().to_string(),
                })?);
            }
            _ => {}
        }
    }

    Ok((
        price_cents.ok_or(FeatureError::MissingField("price_cents"))?,
        size.ok_or(FeatureError::MissingField("size"))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventKind, RawEvent};

    #[test]
    fn computes_tick_feature_update() {
        let mut engine = FeatureEngine::new();
        let first = RawEvent {
            source: "sim".to_string(),
            observed_ns: 1,
            sequence: 1,
            kind: EventKind::Tick,
            symbol: "AAPL".to_string(),
            payload: "price_cents=10000,size=50".to_string(),
        }
        .normalize(1);
        let second = RawEvent {
            source: "sim".to_string(),
            observed_ns: 2,
            sequence: 2,
            kind: EventKind::Tick,
            symbol: "AAPL".to_string(),
            payload: "price_cents=10100,size=25".to_string(),
        }
        .normalize(2);

        assert_eq!(
            engine.process(&first).unwrap().unwrap().previous_price_cents,
            None
        );

        let update = engine.process(&second).unwrap().unwrap();
        assert_eq!(update.previous_price_cents, Some(10000));
        assert_eq!(update.return_bps, Some(100));
        assert_eq!(update.cumulative_size, 75);
    }
}
