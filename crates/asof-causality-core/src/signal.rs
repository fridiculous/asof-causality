use crate::{AsOfView, SymbolId, SymbolSnapshot};

pub trait Signal {
    fn predict(&self, view: AsOfView<'_>, symbol: SymbolId, prediction_time: u64)
        -> SymbolSnapshot;

    /// Stable identifier included in `feature_recipe_hash`.
    fn name(&self) -> &'static str;

    fn config_descriptor(&self) -> String {
        String::new()
    }
}

#[derive(Debug, Default, Clone, Copy)]
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
pub struct WindowedFeatureSentimentSignal {
    window: usize,
}

impl WindowedFeatureSentimentSignal {
    pub const DEFAULT_WINDOW: usize = 5;

    pub fn new(window: usize) -> Self {
        Self {
            window: window.max(1),
        }
    }

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
pub struct WindowedZScoreSignal {
    window: usize,
    threshold: f64,
}

impl WindowedZScoreSignal {
    pub const DEFAULT_WINDOW: usize = 5;
    pub const DEFAULT_THRESHOLD: f64 = 1.0;

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
