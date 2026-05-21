//! Built-in signal implementations and registry for as-of causality replay.

use asof_causality::{
    run_adversarial_checks_with_options_for_signal, run_sensitivity_sweep, AsOfView, CheckOptions,
    CheckReport, Event, FixedDecimal, PolicyPoint, ReplayEngine, ReplayError, ReplayOptions,
    ReplayOrder, ReplayOutput, SensitivityError, SensitivitySweep, Signal, SignalEvaluation,
    SymbolSlot, FIXED_DECIMAL_SCALE,
};
use std::error::Error;
use std::fmt;

/// Built-in signal using the latest received feature sentiment for a symbol.
#[derive(Debug, Default, Clone, Copy)]
pub struct LastFeatureSentimentSignal;

impl Signal for LastFeatureSentimentSignal {
    fn name(&self) -> &'static str {
        "last-feature-sentiment"
    }

    fn evaluate(
        &self,
        view: AsOfView<'_>,
        symbol: SymbolSlot,
        _as_of_timestamp: u64,
    ) -> SignalEvaluation {
        view.snapshot(symbol)
    }
}

/// Built-in signal summing recent feature sentiment over a bounded window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowedFeatureSentimentSignal {
    window: usize,
}

impl WindowedFeatureSentimentSignal {
    /// Default number of recent sentiment features used by the signal.
    pub const DEFAULT_WINDOW: usize = 5;

    /// Creates a windowed sentiment signal with a minimum window of one.
    pub fn new(window: usize) -> Self {
        Self {
            window: window.max(1),
        }
    }

    /// Returns the configured sentiment window size.
    pub fn window(self) -> usize {
        self.window
    }
}

impl Default for WindowedFeatureSentimentSignal {
    fn default() -> Self {
        Self::new(Self::DEFAULT_WINDOW)
    }
}

impl Signal for WindowedFeatureSentimentSignal {
    fn name(&self) -> &'static str {
        "windowed-feature-sentiment"
    }

    fn config_descriptor(&self) -> String {
        format!("window={}", self.window)
    }

    fn evaluate(
        &self,
        view: AsOfView<'_>,
        symbol: SymbolSlot,
        _as_of_timestamp: u64,
    ) -> SignalEvaluation {
        view.windowed_snapshot(symbol, self.window)
    }
}

/// Built-in signal bucketing the latest numeric score by rolling z-score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowedZScoreSignal {
    window: usize,
    threshold_scaled: i64,
}

impl WindowedZScoreSignal {
    /// Default number of recent numeric score features used by the signal.
    pub const DEFAULT_WINDOW: usize = 5;
    /// Default absolute z-score threshold, scaled as a `FixedDecimal`.
    pub const DEFAULT_THRESHOLD_SCALED: i64 = FIXED_DECIMAL_SCALE;

    /// Creates a z-score signal using the default window and threshold.
    pub fn new() -> Self {
        Self {
            window: Self::DEFAULT_WINDOW,
            threshold_scaled: Self::DEFAULT_THRESHOLD_SCALED,
        }
    }
}

impl Default for WindowedZScoreSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl Signal for WindowedZScoreSignal {
    fn name(&self) -> &'static str {
        "windowed-zscore"
    }

    fn config_descriptor(&self) -> String {
        format!(
            "window={};threshold={}",
            self.window,
            FixedDecimal::from_scaled(self.threshold_scaled)
        )
    }

    fn evaluate(
        &self,
        view: AsOfView<'_>,
        symbol: SymbolSlot,
        _as_of_timestamp: u64,
    ) -> SignalEvaluation {
        view.score_window_snapshot(symbol, self.window, self.threshold_scaled)
    }
}

/// Built-in fixed-point fast/slow moving-average crossover signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolAdjustedMomentumSignal {
    fast_window: usize,
    slow_window: usize,
    min_trend: FixedDecimal,
    volatility_divisor: i64,
}

impl VolAdjustedMomentumSignal {
    /// Default fast moving-average window.
    pub const DEFAULT_FAST_WINDOW: usize = 2;
    /// Default slow moving-average window.
    pub const DEFAULT_SLOW_WINDOW: usize = 4;
    /// Default minimum trend gate.
    pub const DEFAULT_MIN_TREND: FixedDecimal = FixedDecimal::from_scaled(0);
    /// Default realized-volatility divisor.
    pub const DEFAULT_VOLATILITY_DIVISOR: i64 = 2;

    /// Creates a momentum signal using default parameters.
    pub fn new() -> Self {
        Self {
            fast_window: Self::DEFAULT_FAST_WINDOW,
            slow_window: Self::DEFAULT_SLOW_WINDOW,
            min_trend: Self::DEFAULT_MIN_TREND,
            volatility_divisor: Self::DEFAULT_VOLATILITY_DIVISOR,
        }
    }
}

impl Default for VolAdjustedMomentumSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl Signal for VolAdjustedMomentumSignal {
    fn name(&self) -> &'static str {
        "vol-adjusted-momentum"
    }

    fn config_descriptor(&self) -> String {
        format!(
            "fast_window={};slow_window={};min_trend={};volatility_divisor={}",
            self.fast_window, self.slow_window, self.min_trend, self.volatility_divisor
        )
    }

    fn evaluate(
        &self,
        view: AsOfView<'_>,
        symbol: SymbolSlot,
        _as_of_timestamp: u64,
    ) -> SignalEvaluation {
        view.score_momentum_snapshot(
            symbol,
            self.fast_window,
            self.slow_window,
            self.min_trend,
            self.volatility_divisor,
        )
    }
}

/// Stable names for the built-in signals linked into the CLI.
pub const SIGNAL_NAMES: &[&str] = &[
    "last-feature-sentiment",
    "windowed-feature-sentiment",
    "windowed-zscore",
    "vol-adjusted-momentum",
];

/// Built-in signal registry entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RegisteredSignal {
    /// Latest received feature sentiment.
    #[default]
    LastFeatureSentiment,
    /// Bounded recent sentiment window.
    WindowedFeatureSentiment,
    /// Bounded numeric rolling z-score.
    WindowedZScore,
    /// Fixed-point volatility-adjusted momentum crossover.
    VolAdjustedMomentum,
}

impl RegisteredSignal {
    /// Parses a registry signal name.
    pub fn parse(value: &str) -> Result<Self, ParseSignalError> {
        match value {
            "last-feature-sentiment" => Ok(Self::LastFeatureSentiment),
            "windowed-feature-sentiment" => Ok(Self::WindowedFeatureSentiment),
            "windowed-zscore" => Ok(Self::WindowedZScore),
            "vol-adjusted-momentum" => Ok(Self::VolAdjustedMomentum),
            other => Err(ParseSignalError {
                unknown: other.to_string(),
            }),
        }
    }

    /// Returns the stable signal name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LastFeatureSentiment => "last-feature-sentiment",
            Self::WindowedFeatureSentiment => "windowed-feature-sentiment",
            Self::WindowedZScore => "windowed-zscore",
            Self::VolAdjustedMomentum => "vol-adjusted-momentum",
        }
    }

    /// Returns the default configuration descriptor for this registered signal.
    pub fn config_descriptor(self) -> String {
        match self {
            Self::LastFeatureSentiment => LastFeatureSentimentSignal.config_descriptor(),
            Self::WindowedFeatureSentiment => {
                WindowedFeatureSentimentSignal::default().config_descriptor()
            }
            Self::WindowedZScore => WindowedZScoreSignal::default().config_descriptor(),
            Self::VolAdjustedMomentum => VolAdjustedMomentumSignal::default().config_descriptor(),
        }
    }

    /// Returns all known signal names.
    pub fn names() -> &'static [&'static str] {
        SIGNAL_NAMES
    }

    /// Replays this signal over events with the requested replay order.
    pub fn replay(
        self,
        events: &[Event],
        options: ReplayOptions,
        order: ReplayOrder,
    ) -> Result<ReplayOutput, ReplayError> {
        match self {
            Self::LastFeatureSentiment => ReplayEngine::with_signal(LastFeatureSentimentSignal)
                .replay_with_order(events, options, order),
            Self::WindowedFeatureSentiment => {
                ReplayEngine::with_signal(WindowedFeatureSentimentSignal::default())
                    .replay_with_order(events, options, order)
            }
            Self::WindowedZScore => ReplayEngine::with_signal(WindowedZScoreSignal::default())
                .replay_with_order(events, options, order),
            Self::VolAdjustedMomentum => {
                ReplayEngine::with_signal(VolAdjustedMomentumSignal::default())
                    .replay_with_order(events, options, order)
            }
        }
    }

    /// Runs the adversarial check suite for this signal.
    pub fn check(self, events: &[Event], options: CheckOptions) -> CheckReport {
        match self {
            Self::LastFeatureSentiment => run_adversarial_checks_with_options_for_signal(
                events,
                options,
                LastFeatureSentimentSignal,
            ),
            Self::WindowedFeatureSentiment => run_adversarial_checks_with_options_for_signal(
                events,
                options,
                WindowedFeatureSentimentSignal::default(),
            ),
            Self::WindowedZScore => run_adversarial_checks_with_options_for_signal(
                events,
                options,
                WindowedZScoreSignal::default(),
            ),
            Self::VolAdjustedMomentum => run_adversarial_checks_with_options_for_signal(
                events,
                options,
                VolAdjustedMomentumSignal::default(),
            ),
        }
    }

    /// Runs the sensitivity sweep for this signal.
    pub fn sensitivity(
        self,
        events: &[Event],
        policies: &[PolicyPoint],
    ) -> Result<SensitivitySweep, SensitivityError> {
        match self {
            Self::LastFeatureSentiment => {
                run_sensitivity_sweep(events, policies, LastFeatureSentimentSignal)
            }
            Self::WindowedFeatureSentiment => {
                run_sensitivity_sweep(events, policies, WindowedFeatureSentimentSignal::default())
            }
            Self::WindowedZScore => {
                run_sensitivity_sweep(events, policies, WindowedZScoreSignal::default())
            }
            Self::VolAdjustedMomentum => {
                run_sensitivity_sweep(events, policies, VolAdjustedMomentumSignal::default())
            }
        }
    }
}

/// Error returned for an unknown signal name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseSignalError {
    unknown: String,
}

impl fmt::Display for ParseSignalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown signal {}; expected {}",
            self.unknown,
            SIGNAL_NAMES.join(", ")
        )
    }
}

impl Error for ParseSignalError {}

#[cfg(test)]
mod tests {
    use super::*;
    use asof_causality::{parse_pipe_events, ReplayOptions};

    #[test]
    fn parses_registered_signal_names() {
        assert_eq!(
            RegisteredSignal::parse("windowed-zscore").unwrap(),
            RegisteredSignal::WindowedZScore
        );
        assert_eq!(
            RegisteredSignal::parse("vol-adjusted-momentum").unwrap(),
            RegisteredSignal::VolAdjustedMomentum
        );
        assert!(RegisteredSignal::parse("missing").is_err());
    }

    #[test]
    fn windowed_signal_records_multiple_inputs() {
        let events =
            parse_pipe_events(include_str!("../../../examples/late-arrival.pipe")).unwrap();
        let output = ReplayEngine::with_signal(WindowedFeatureSentimentSignal::new(5))
            .replay(&events, ReplayOptions::default())
            .unwrap();

        assert!(output
            .predictions
            .records()
            .iter()
            .any(|record| record.input_event_ids_used.len() > 1));
    }

    #[test]
    fn zscore_signal_runs_real_data_fixture() {
        let events =
            parse_pipe_events(include_str!("../../../examples/alfred-dgs10-sp500.pipe")).unwrap();
        let output = RegisteredSignal::WindowedZScore
            .replay(&events, ReplayOptions::default(), ReplayOrder::ReceivedTime)
            .unwrap();

        assert_eq!(output.predictions.records().len(), 4);
    }

    #[test]
    fn sensitivity_runs_through_registry() {
        let events = parse_pipe_events(include_str!(
            "../../../examples/lookahead-negative-control.pipe"
        ))
        .unwrap();
        let policies = [PolicyPoint::observed_time_leaky()];
        let sweep = RegisteredSignal::WindowedFeatureSentiment
            .sensitivity(&events, &policies)
            .unwrap();

        assert_eq!(sweep.results.len(), 1);
    }
}
