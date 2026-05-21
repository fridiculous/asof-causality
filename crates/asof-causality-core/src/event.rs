use crate::{EventKey, SymbolId};
use std::error::Error;
use std::fmt;
use std::str::FromStr;

pub const FIXED_DECIMAL_SCALE: i64 = 1_000_000;
pub const FIXED_DECIMAL_SCALE_DIGITS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureDType {
    FixedDecimal { scale: usize },
    Int64,
    Bool,
    Text,
}

impl fmt::Display for FeatureDType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FixedDecimal { scale } => write!(f, "fixed_decimal(scale={scale})"),
            Self::Int64 => write!(f, "int64"),
            Self::Bool => write!(f, "bool"),
            Self::Text => write!(f, "text"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureSpec {
    pub name: &'static str,
    pub dtype: FeatureDType,
}

impl FeatureSpec {
    pub const fn new(name: &'static str, dtype: FeatureDType) -> Self {
        Self { name, dtype }
    }
}

pub const SENTIMENT_FEATURE: FeatureSpec = FeatureSpec::new("sentiment", FeatureDType::Text);
pub const SCORE_FEATURE: FeatureSpec = FeatureSpec::new(
    "score",
    FeatureDType::FixedDecimal {
        scale: FIXED_DECIMAL_SCALE_DIGITS,
    },
);
pub const BUILTIN_FEATURE_SPECS: [FeatureSpec; 2] = [SENTIMENT_FEATURE, SCORE_FEATURE];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Role of an event in the causality replay stream.
pub enum EventRole {
    /// Feature information that signal code may use after its received time.
    Feature,
    /// Append-only correction to earlier feature information.
    FeatureCorrection,
    /// Prediction point where the replay engine asks a signal for a value.
    Prediction,
    /// Future evaluation data that must not affect signal state.
    Outcome,
}

impl EventRole {
    /// Returns the canonical pipe-format name for this role.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Feature => "feature",
            Self::FeatureCorrection => "feature_correction",
            Self::Prediction => "prediction",
            Self::Outcome => "outcome",
        }
    }

    /// Returns whether this role can update signal-visible state.
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
/// Discrete feature sentiment used by built-in example signals.
pub enum Sentiment {
    /// Negative sentiment, mapped to signal value `-1`.
    Negative,
    /// Neutral sentiment, mapped to signal value `0`.
    Neutral,
    /// Positive sentiment, mapped to signal value `1`.
    Positive,
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// Parsed feature payload values for signal-state updates.
pub struct FeatureValues {
    /// Optional discrete sentiment value.
    pub sentiment: Option<Sentiment>,
    /// Optional numeric score value.
    pub score: Option<FixedDecimal>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
/// Deterministic fixed-point decimal with six fractional digits.
pub struct FixedDecimal {
    scaled: i64,
}

impl FixedDecimal {
    /// Fixed scale used by `FixedDecimal`.
    pub const SCALE: i64 = FIXED_DECIMAL_SCALE;

    /// Builds a fixed decimal from its already-scaled integer representation.
    pub const fn from_scaled(scaled: i64) -> Self {
        Self { scaled }
    }

    /// Returns the scaled integer representation.
    pub const fn scaled(self) -> i64 {
        self.scaled
    }

    /// Returns the absolute value, saturating at `i64::MAX`.
    pub fn abs(self) -> Self {
        Self {
            scaled: self.scaled.saturating_abs(),
        }
    }
}

impl fmt::Display for FixedDecimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.scaled < 0 { "-" } else { "" };
        let magnitude = i128::from(self.scaled).abs();
        let whole = magnitude / i128::from(FIXED_DECIMAL_SCALE);
        let mut fractional = format!(
            "{:0width$}",
            magnitude % i128::from(FIXED_DECIMAL_SCALE),
            width = FIXED_DECIMAL_SCALE_DIGITS
        );

        while fractional.ends_with('0') {
            fractional.pop();
        }

        if fractional.is_empty() {
            write!(f, "{sign}{whole}")
        } else {
            write!(f, "{sign}{whole}.{fractional}")
        }
    }
}

impl Sentiment {
    /// Converts sentiment into the built-in prediction signal value.
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
/// Two-clock event row consumed by the replay engine.
pub struct Event {
    /// Human-readable event identifier from the input stream.
    pub event_id: String,
    /// Stable compact key derived from `event_id`.
    pub event_key: EventKey,
    /// Time the event was observed in the source domain.
    pub observed_time: u64,
    /// Time the event became available to the replay engine.
    pub received_time: u64,
    /// Tie-breaker within the replay timestamp.
    pub sequence: u64,
    /// Event role controlling how replay handles the row.
    pub role: EventRole,
    /// Stable compact key derived from `symbol`.
    pub symbol_key: SymbolId,
    /// Human-readable symbol label.
    pub symbol: String,
    /// Opaque comma-delimited payload parsed by accessors.
    pub payload: String,
}

impl Event {
    /// Builds an event and derives stable event and symbol keys.
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

    /// Parses one pipe-delimited event record.
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

    /// Serializes the event back to the pipe-delimited fixture format.
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

    /// Returns the correct replay ordering key.
    pub fn replay_key(&self) -> (u64, u64, &str) {
        (self.received_time, self.sequence, self.event_id.as_str())
    }

    /// Returns the deliberately leaky observed-time ordering key.
    pub fn observed_key(&self) -> (u64, u64, &str) {
        (self.observed_time, self.sequence, self.event_id.as_str())
    }

    /// Parses the optional `sentiment` payload field for feature roles.
    pub fn sentiment(&self) -> Result<Option<Sentiment>, ParseEventError> {
        if !self.role.updates_signal_state() {
            return Ok(None);
        }

        payload_field(&self.payload, SENTIMENT_FEATURE.name)
            .map(|value| value.parse().map(Some))
            .unwrap_or(Ok(None))
    }

    /// Parses the optional `score` payload field for feature roles.
    pub fn score(&self) -> Result<Option<FixedDecimal>, ParseEventError> {
        if !self.role.updates_signal_state() {
            return Ok(None);
        }

        payload_field(&self.payload, SCORE_FEATURE.name)
            .map(|value| parse_fixed_decimal(SCORE_FEATURE.name, value).map(Some))
            .unwrap_or(Ok(None))
    }

    /// Parses all signal-state feature values from the payload.
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

    /// Returns a copy with future feature payloads changed for mutation checks.
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
/// Error returned while parsing event rows or feature payloads.
pub enum ParseEventError {
    /// The input record is empty.
    Empty,
    /// A required field was empty.
    MissingField {
        /// Name of the missing field.
        field: &'static str,
    },
    /// The input record had the wrong number of pipe-delimited fields.
    WrongFieldCount {
        /// Expected field count.
        expected: usize,
        /// Actual field count.
        actual: usize,
    },
    /// A numeric field could not be parsed.
    InvalidNumber {
        /// Name of the numeric field.
        field: &'static str,
        /// Rejected value.
        value: String,
    },
    /// The role field was not recognized.
    InvalidRole(String),
    /// The sentiment value was not recognized.
    InvalidSentiment(String),
    /// Two different symbol labels resolved to the same stable symbol id.
    SymbolIdCollision {
        /// Stable id shared by both labels.
        symbol_id: SymbolId,
        /// First label registered for the id.
        existing_symbol: String,
        /// Later label that collided with the first label.
        conflicting_symbol: String,
    },
    /// The same symbol label appeared with a different stable id.
    SymbolIdentityMismatch {
        /// Human-readable symbol label.
        symbol: String,
        /// Id registered when the label was first interned.
        expected: SymbolId,
        /// Id observed on a later event for the same label.
        actual: SymbolId,
    },
    /// Defensive error for querying a catalog with a symbol it did not intern.
    UnknownSymbol {
        /// Human-readable symbol label.
        symbol: String,
        /// Stable id from the lookup event.
        symbol_id: SymbolId,
    },
    /// A feature event did not contain any supported feature value.
    MissingPayloadField {
        /// Event identifier for the malformed row.
        event_id: String,
        /// Missing payload field name.
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
            Self::SymbolIdCollision {
                symbol_id,
                existing_symbol,
                conflicting_symbol,
            } => write!(
                f,
                "symbol id collision for {:016x}: {existing_symbol} conflicts with {conflicting_symbol}",
                symbol_id.0
            ),
            Self::SymbolIdentityMismatch {
                symbol,
                expected,
                actual,
            } => write!(
                f,
                "symbol {symbol} changed identity: expected {:016x}, found {:016x}",
                expected.0, actual.0
            ),
            Self::UnknownSymbol { symbol, symbol_id } => write!(
                f,
                "symbol {symbol} with id {:016x} was not present in the symbol catalog",
                symbol_id.0
            ),
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

fn parse_fixed_decimal(field: &'static str, value: &str) -> Result<FixedDecimal, ParseEventError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ParseEventError::InvalidNumber {
            field,
            value: trimmed.to_string(),
        });
    }

    let (sign, unsigned) = match trimmed.as_bytes()[0] {
        b'-' => (-1_i128, &trimmed[1..]),
        b'+' => (1_i128, &trimmed[1..]),
        _ => (1_i128, trimmed),
    };

    let (whole, fractional) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if (whole.is_empty() && fractional.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(ParseEventError::InvalidNumber {
            field,
            value: trimmed.to_string(),
        });
    }

    let whole_value = if whole.is_empty() {
        0_i128
    } else {
        whole
            .parse::<i128>()
            .map_err(|_| ParseEventError::InvalidNumber {
                field,
                value: trimmed.to_string(),
            })?
    };

    let mut fractional_value = 0_i128;
    let kept_digits = fractional.len().min(FIXED_DECIMAL_SCALE_DIGITS);
    for byte in fractional.bytes().take(kept_digits) {
        fractional_value = fractional_value * 10 + i128::from(byte - b'0');
    }
    for _ in kept_digits..FIXED_DECIMAL_SCALE_DIGITS {
        fractional_value *= 10;
    }

    if fractional
        .as_bytes()
        .get(FIXED_DECIMAL_SCALE_DIGITS)
        .is_some_and(|byte| *byte >= b'5')
    {
        fractional_value += 1;
    }

    let magnitude = whole_value
        .checked_mul(i128::from(FIXED_DECIMAL_SCALE))
        .and_then(|value| value.checked_add(fractional_value))
        .ok_or_else(|| ParseEventError::InvalidNumber {
            field,
            value: trimmed.to_string(),
        })?;

    let scaled = magnitude
        .checked_mul(sign)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| ParseEventError::InvalidNumber {
            field,
            value: trimmed.to_string(),
        })?;

    Ok(FixedDecimal::from_scaled(scaled))
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
        assert_eq!(
            event.score().unwrap(),
            Some(FixedDecimal::from_scaled(730_000))
        );
        assert_eq!(
            event.feature_values().unwrap().unwrap().score,
            Some(FixedDecimal::from_scaled(730_000))
        );
    }

    #[test]
    fn parses_score_payload_as_rounded_fixed_point() {
        let event = Event::from_pipe_record("px1|1|1|1|feature|AAPL|score=-0.1234567").unwrap();

        assert_eq!(
            event.score().unwrap(),
            Some(FixedDecimal::from_scaled(-123_457))
        );
        assert_eq!(event.score().unwrap().unwrap().to_string(), "-0.123457");
    }

    #[test]
    fn declares_builtin_feature_dtypes() {
        assert_eq!(SENTIMENT_FEATURE.name, "sentiment");
        assert_eq!(SENTIMENT_FEATURE.dtype, FeatureDType::Text);
        assert_eq!(SCORE_FEATURE.name, "score");
        assert_eq!(
            SCORE_FEATURE.dtype,
            FeatureDType::FixedDecimal {
                scale: FIXED_DECIMAL_SCALE_DIGITS
            }
        );
        assert_eq!(BUILTIN_FEATURE_SPECS, [SENTIMENT_FEATURE, SCORE_FEATURE]);
        assert_eq!(SCORE_FEATURE.dtype.to_string(), "fixed_decimal(scale=6)");
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
