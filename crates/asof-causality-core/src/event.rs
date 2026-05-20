use crate::{EventKey, SymbolId};
use std::error::Error;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventRole {
    Feature,
    FeatureCorrection,
    Prediction,
    Outcome,
}

impl EventRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Feature => "feature",
            Self::FeatureCorrection => "feature_correction",
            Self::Prediction => "prediction",
            Self::Outcome => "outcome",
        }
    }

    pub fn updates_signal_state(self) -> bool {
        matches!(self, Self::Feature | Self::FeatureCorrection)
    }
}

impl FromStr for EventRole {
    type Err = ParseEventError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "feature" | "news" => Ok(Self::Feature),
            "feature_correction" | "correction" => Ok(Self::FeatureCorrection),
            "prediction" | "predict" => Ok(Self::Prediction),
            "outcome" | "label" => Ok(Self::Outcome),
            other => Err(ParseEventError::InvalidRole(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sentiment {
    Negative,
    Neutral,
    Positive,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeatureValues {
    pub sentiment: Option<Sentiment>,
    pub score: Option<f64>,
}

impl Sentiment {
    pub fn signal_value(self) -> i8 {
        match self {
            Self::Negative => -1,
            Self::Neutral => 0,
            Self::Positive => 1,
        }
    }
}

impl FromStr for Sentiment {
    type Err = ParseEventError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "negative" | "-1" => Ok(Self::Negative),
            "neutral" | "0" => Ok(Self::Neutral),
            "positive" | "1" => Ok(Self::Positive),
            other => Err(ParseEventError::InvalidSentiment(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub event_id: String,
    pub event_key: EventKey,
    pub observed_time: u64,
    pub received_time: u64,
    pub sequence: u64,
    pub role: EventRole,
    pub symbol_key: SymbolId,
    pub symbol: String,
    pub payload: String,
}

impl Event {
    pub fn new(
        event_id: impl Into<String>,
        observed_time: u64,
        received_time: u64,
        sequence: u64,
        role: EventRole,
        symbol: impl Into<String>,
        payload: impl Into<String>,
    ) -> Self {
        let event_id = event_id.into();
        let symbol = symbol.into();
        Self {
            event_key: EventKey::from_label(&event_id),
            event_id,
            observed_time,
            received_time,
            sequence,
            role,
            symbol_key: SymbolId::from_label(&symbol),
            symbol,
            payload: payload.into(),
        }
    }

    pub fn from_pipe_record(line: &str) -> Result<Self, ParseEventError> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Err(ParseEventError::Empty);
        }

        let fields: Vec<&str> = trimmed.splitn(7, '|').collect();
        if fields.len() != 7 {
            return Err(ParseEventError::WrongFieldCount {
                expected: 7,
                actual: fields.len(),
            });
        }

        let event_id = required("event_id", fields[0])?.to_string();

        Ok(Self::new(
            event_id,
            parse_u64("observed_time", fields[1])?,
            parse_u64("received_time", fields[2])?,
            parse_u64("sequence", fields[3])?,
            fields[4].parse()?,
            required("symbol", fields[5])?,
            fields[6].trim(),
        ))
    }

    pub fn to_pipe_record(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            self.event_id,
            self.observed_time,
            self.received_time,
            self.sequence,
            self.role.as_str(),
            self.symbol,
            self.payload
        )
    }

    pub fn replay_key(&self) -> (u64, u64, &str) {
        (self.received_time, self.sequence, self.event_id.as_str())
    }

    pub fn observed_key(&self) -> (u64, u64, &str) {
        (self.observed_time, self.sequence, self.event_id.as_str())
    }

    pub fn sentiment(&self) -> Result<Option<Sentiment>, ParseEventError> {
        if !self.role.updates_signal_state() {
            return Ok(None);
        }

        payload_field(&self.payload, "sentiment")
            .map(|value| value.parse().map(Some))
            .unwrap_or(Ok(None))
    }

    pub fn score(&self) -> Result<Option<f64>, ParseEventError> {
        if !self.role.updates_signal_state() {
            return Ok(None);
        }

        payload_field(&self.payload, "score")
            .map(|value| parse_f64("score", value).map(Some))
            .unwrap_or(Ok(None))
    }

    pub fn feature_values(&self) -> Result<Option<FeatureValues>, ParseEventError> {
        if !self.role.updates_signal_state() {
            return Ok(None);
        }

        let values = FeatureValues {
            sentiment: self.sentiment()?,
            score: self.score()?,
        };

        if values.sentiment.is_none() && values.score.is_none() {
            return Err(ParseEventError::MissingPayloadField {
                event_id: self.event_id.clone(),
                field: "sentiment_or_score",
            });
        }

        Ok(Some(values))
    }

    pub fn with_mutated_future_payload(&self) -> Self {
        let mut mutated = self.clone();
        if mutated.role.updates_signal_state() {
            if payload_field(&mutated.payload, "score").is_some() {
                mutated.payload = "score=-999,mutated=true".to_string();
            } else {
                mutated.payload = "sentiment=negative,mutated=true".to_string();
            }
        } else {
            mutated.payload = format!("{},mutated=true", mutated.payload);
        }
        mutated
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseEventError {
    Empty,
    MissingField {
        field: &'static str,
    },
    WrongFieldCount {
        expected: usize,
        actual: usize,
    },
    InvalidNumber {
        field: &'static str,
        value: String,
    },
    InvalidRole(String),
    InvalidSentiment(String),
    MissingPayloadField {
        event_id: String,
        field: &'static str,
    },
}

impl fmt::Display for ParseEventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "event line is empty"),
            Self::MissingField { field } => write!(f, "missing required field: {field}"),
            Self::WrongFieldCount { expected, actual } => {
                write!(f, "expected {expected} fields, found {actual}")
            }
            Self::InvalidNumber { field, value } => {
                write!(f, "invalid numeric field {field}: {value}")
            }
            Self::InvalidRole(role) => write!(f, "invalid event role: {role}"),
            Self::InvalidSentiment(value) => write!(f, "invalid sentiment: {value}"),
            Self::MissingPayloadField { event_id, field } => {
                write!(f, "event {event_id} is missing payload field {field}")
            }
        }
    }
}

impl Error for ParseEventError {}

fn required<'a>(field: &'static str, value: &'a str) -> Result<&'a str, ParseEventError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(ParseEventError::MissingField { field })
    } else {
        Ok(trimmed)
    }
}

fn parse_u64(field: &'static str, value: &str) -> Result<u64, ParseEventError> {
    value
        .trim()
        .parse()
        .map_err(|_| ParseEventError::InvalidNumber {
            field,
            value: value.trim().to_string(),
        })
}

fn parse_f64(field: &'static str, value: &str) -> Result<f64, ParseEventError> {
    value
        .trim()
        .parse()
        .map_err(|_| ParseEventError::InvalidNumber {
            field,
            value: value.trim().to_string(),
        })
}

fn payload_field<'a>(payload: &'a str, field: &str) -> Option<&'a str> {
    payload.split(',').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key.trim() == field).then_some(value.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pipe_record() {
        let event =
            Event::from_pipe_record("n1|572|585|2|feature|AAPL|sentiment=positive").unwrap();

        assert_eq!(event.event_id, "n1");
        assert_eq!(event.observed_time, 572);
        assert_eq!(event.received_time, 585);
        assert_eq!(event.role, EventRole::Feature);
        assert_eq!(event.sentiment().unwrap(), Some(Sentiment::Positive));
    }

    #[test]
    fn parses_numeric_score_payload() {
        let event = Event::from_pipe_record("px1|1|1|1|feature|AAPL|score=0.73").unwrap();

        assert_eq!(event.sentiment().unwrap(), None);
        assert_eq!(event.score().unwrap(), Some(0.73));
        assert_eq!(event.feature_values().unwrap().unwrap().score, Some(0.73));
    }

    #[test]
    fn parses_legacy_role_aliases() {
        assert_eq!(
            Event::from_pipe_record("n1|1|1|1|news|AAPL|sentiment=positive")
                .unwrap()
                .role,
            EventRole::Feature
        );
        assert_eq!(
            Event::from_pipe_record("c1|1|1|1|correction|AAPL|sentiment=negative")
                .unwrap()
                .role,
            EventRole::FeatureCorrection
        );
        assert_eq!(
            Event::from_pipe_record("p1|1|1|1|predict|AAPL|")
                .unwrap()
                .role,
            EventRole::Prediction
        );
        assert_eq!(
            Event::from_pipe_record("l1|1|1|1|label|AAPL|return_bps=1")
                .unwrap()
                .role,
            EventRole::Outcome
        );
    }

    #[test]
    fn rejects_unknown_role() {
        let error = Event::from_pipe_record("x|1|1|1|bad|AAPL|").unwrap_err();
        assert!(matches!(error, ParseEventError::InvalidRole(_)));
    }
}
