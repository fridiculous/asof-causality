use crate::{AsOfView, FixedDecimal, SignalEvaluation, SymbolSlot, FIXED_DECIMAL_SCALE};

/// Signal implementation evaluated by the replay engine at prediction events.
pub trait Signal {
    /// Evaluates signal state from the opaque as-of view.
    fn evaluate(
        &self,
        view: AsOfView<'_>,
        symbol: SymbolSlot,
        as_of_timestamp: u64,
    ) -> SignalEvaluation;

    /// Stable signal name used in audit records and recipe hashes.
    fn name(&self) -> &'static str;

    /// Stable configuration descriptor included in recipe hashes.
    fn config_descriptor(&self) -> String {
        String::new()
    }
}

#[derive(Debug, Default, Clone, Copy)]
/// Built-in signal using the latest received feature sentiment for a symbol.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Built-in signal summing recent feature sentiment over a bounded window.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Built-in signal bucketing the latest numeric score by rolling z-score.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Built-in fixed-point fast/slow moving-average crossover signal.
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
