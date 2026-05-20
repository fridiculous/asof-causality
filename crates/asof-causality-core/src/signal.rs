use crate::{AsOfView, SymbolId, SymbolSnapshot};

/// Signal implementation evaluated by the replay engine at prediction events.
pub trait Signal {
    /// Computes a prediction from the opaque as-of view.
    fn predict(&self, view: AsOfView<'_>, symbol: SymbolId, prediction_time: u64)
        -> SymbolSnapshot;

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

    fn predict(
        &self,
        view: AsOfView<'_>,
        symbol: SymbolId,
        _prediction_time: u64,
    ) -> SymbolSnapshot {
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

    fn predict(
        &self,
        view: AsOfView<'_>,
        symbol: SymbolId,
        _prediction_time: u64,
    ) -> SymbolSnapshot {
        view.windowed_snapshot(symbol, self.window)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// Built-in signal bucketing the latest numeric score by rolling z-score.
pub struct WindowedZScoreSignal {
    window: usize,
    threshold: f64,
}

impl WindowedZScoreSignal {
    /// Default number of recent numeric score features used by the signal.
    pub const DEFAULT_WINDOW: usize = 5;
    /// Default absolute z-score threshold for emitting non-zero values.
    pub const DEFAULT_THRESHOLD: f64 = 1.0;

    /// Creates a z-score signal using the default window and threshold.
    pub fn new() -> Self {
        Self {
            window: Self::DEFAULT_WINDOW,
            threshold: Self::DEFAULT_THRESHOLD,
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
        format!("window={};threshold={}", self.window, self.threshold)
    }

    fn predict(
        &self,
        view: AsOfView<'_>,
        symbol: SymbolId,
        _prediction_time: u64,
    ) -> SymbolSnapshot {
        view.score_window_snapshot(symbol, self.window, self.threshold)
    }
}
