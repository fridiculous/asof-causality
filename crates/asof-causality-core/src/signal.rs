use crate::{AsOfView, SymbolId, SymbolSnapshot};

pub trait Signal {
    fn predict(&self, view: AsOfView<'_>, symbol: SymbolId, prediction_time: u64)
        -> SymbolSnapshot;

    fn name(&self) -> &'static str {
        "custom-signal"
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

    fn predict(
        &self,
        view: AsOfView<'_>,
        symbol: SymbolId,
        _prediction_time: u64,
    ) -> SymbolSnapshot {
        view.windowed_snapshot(symbol, self.window)
    }
}
