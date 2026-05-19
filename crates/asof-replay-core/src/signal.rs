use crate::{AsOfView, SymbolSnapshot};

pub trait Signal {
    fn predict(&self, view: AsOfView<'_>, symbol: &str, prediction_time: u64) -> SymbolSnapshot;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LastSentimentSignal;

impl Signal for LastSentimentSignal {
    fn predict(&self, view: AsOfView<'_>, symbol: &str, _prediction_time: u64) -> SymbolSnapshot {
        view.snapshot(symbol)
    }
}
