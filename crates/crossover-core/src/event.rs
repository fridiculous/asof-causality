use std::error::Error;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Tick,
    News,
    Filing,
    AltData,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tick => "tick",
            Self::News => "news",
            Self::Filing => "filing",
            Self::AltData => "alt_data",
        }
    }
}

impl FromStr for EventKind {
    type Err = ParseEventError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "tick" => Ok(Self::Tick),
            "news" => Ok(Self::News),
            "filing" => Ok(Self::Filing),
            "alt_data" => Ok(Self::AltData),
            other => Err(ParseEventError::InvalidKind(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEvent {
    pub source: String,
    pub observed_ns: u64,
    pub sequence: u64,
    pub kind: EventKind,
    pub symbol: String,
    pub payload: String,
}

impl RawEvent {
    pub fn from_pipe_record(line: &str) -> Result<Self, ParseEventError> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Err(ParseEventError::Empty);
        }

        let fields: Vec<&str> = trimmed.splitn(6, '|').collect();
        if fields.len() != 6 {
            return Err(ParseEventError::WrongFieldCount {
                expected: 6,
                actual: fields.len(),
            });
        }

        let observed_ns = parse_u64("observed_ns", fields[1])?;
        let sequence = parse_u64("sequence", fields[2])?;
        let kind = fields[3].parse()?;

        Ok(Self {
            source: fields[0].trim().to_string(),
            observed_ns,
            sequence,
            kind,
            symbol: fields[4].trim().to_string(),
            payload: fields[5].trim().to_string(),
        })
    }

    pub fn normalize(self, received_ns: u64) -> NormalizedEvent {
        NormalizedEvent {
            source: self.source,
            observed_ns: self.observed_ns,
            received_ns,
            sequence: self.sequence,
            kind: self.kind,
            symbol: self.symbol,
            payload: self.payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedEvent {
    pub source: String,
    pub observed_ns: u64,
    pub received_ns: u64,
    pub sequence: u64,
    pub kind: EventKind,
    pub symbol: String,
    pub payload: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseEventError {
    Empty,
    WrongFieldCount {
        expected: usize,
        actual: usize,
    },
    InvalidNumber {
        field: &'static str,
        value: String,
    },
    InvalidKind(String),
}

impl fmt::Display for ParseEventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "event line is empty"),
            Self::WrongFieldCount { expected, actual } => {
                write!(f, "expected {expected} fields, found {actual}")
            }
            Self::InvalidNumber { field, value } => {
                write!(f, "invalid numeric field {field}: {value}")
            }
            Self::InvalidKind(kind) => write!(f, "invalid event kind: {kind}"),
        }
    }
}

impl Error for ParseEventError {}

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
        let event =
            RawEvent::from_pipe_record("sim|100|7|tick|AAPL|price_cents=123,size=4").unwrap();

        assert_eq!(event.source, "sim");
        assert_eq!(event.observed_ns, 100);
        assert_eq!(event.sequence, 7);
        assert_eq!(event.kind, EventKind::Tick);
        assert_eq!(event.symbol, "AAPL");
    }

    #[test]
    fn rejects_unknown_kind() {
        let error = RawEvent::from_pipe_record("sim|100|7|bad|AAPL|payload").unwrap_err();
        assert!(matches!(error, ParseEventError::InvalidKind(_)));
    }
}
