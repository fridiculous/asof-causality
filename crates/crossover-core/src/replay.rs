use crate::{ParseEventError, RawEvent};
use std::error::Error;
use std::fmt;

pub fn parse_pipe_events(input: &str) -> Result<Vec<RawEvent>, ReplayError> {
    let mut events = Vec::new();

    for (index, line) in input.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let event = RawEvent::from_pipe_record(trimmed).map_err(|source| ReplayError {
            line: index + 1,
            source,
        })?;
        events.push(event);
    }

    Ok(events)
}

#[derive(Debug)]
pub struct ReplayError {
    pub line: usize,
    pub source: ParseEventError,
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "failed to parse replay line {}: {}", self.line, self.source)
    }
}

impl Error for ReplayError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_comments_and_blank_lines() {
        let input = "\n# comment\nsim|1|1|tick|AAPL|price_cents=1,size=1\n";
        let events = parse_pipe_events(input).unwrap();
        assert_eq!(events.len(), 1);
    }
}
