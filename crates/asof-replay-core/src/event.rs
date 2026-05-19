use crate::EventKey;
use std::error::Error;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    News,
    Correction,
    Predict,
    Label,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::News => "news",
            Self::Correction => "correction",
            Self::Predict => "predict",
            Self::Label => "label",
        }
    }

    pub fn updates_prediction_state(self) -> bool {
        matches!(self, Self::News | Self::Correction)
    }
}

impl FromStr for EventKind {
    type Err = ParseEventError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "news" => Ok(Self::News),
            "correction" => Ok(Self::Correction),
            "predict" => Ok(Self::Predict),
            "label" => Ok(Self::Label),
            other => Err(ParseEventError::InvalidKind(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sentiment {
    Negative,
    Neutral,
    Positive,
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
    pub kind: EventKind,
    pub symbol: String,
    pub payload: String,
}

impl Event {
    pub fn new(
        event_id: impl Into<String>,
        observed_time: u64,
        received_time: u64,
        sequence: u64,
        kind: EventKind,
        symbol: impl Into<String>,
        payload: impl Into<String>,
    ) -> Self {
        let event_id = event_id.into();
        Self {
            event_key: EventKey::from_label(&event_id),
            event_id,
            observed_time,
            received_time,
            sequence,
            kind,
            symbol: symbol.into(),
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
            self.kind.as_str(),
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
        if !self.kind.updates_prediction_state() {
            return Ok(None);
        }

        for pair in self.payload.split(',') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            if key.trim() == "sentiment" {
                return value.trim().parse().map(Some);
            }
        }

        Err(ParseEventError::MissingPayloadField {
            event_id: self.event_id.clone(),
            field: "sentiment",
        })
    }

    pub fn with_mutated_future_payload(&self) -> Self {
        let mut mutated = self.clone();
        if mutated.kind.updates_prediction_state() {
            mutated.payload = "sentiment=negative,mutated=true".to_string();
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
    InvalidKind(String),
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
            Self::InvalidKind(kind) => write!(f, "invalid event kind: {kind}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pipe_record() {
        let event = Event::from_pipe_record("n1|572|585|2|news|AAPL|sentiment=positive").unwrap();

        assert_eq!(event.event_id, "n1");
        assert_eq!(event.observed_time, 572);
        assert_eq!(event.received_time, 585);
        assert_eq!(event.kind, EventKind::News);
        assert_eq!(event.sentiment().unwrap(), Some(Sentiment::Positive));
    }

    #[test]
    fn rejects_unknown_kind() {
        let error = Event::from_pipe_record("x|1|1|1|bad|AAPL|").unwrap_err();
        assert!(matches!(error, ParseEventError::InvalidKind(_)));
    }
}
