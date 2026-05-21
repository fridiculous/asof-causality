use crate::{AsOfView, SignalEvaluation, SymbolSlot};

/// Signal implementation evaluated by the replay engine at prediction events.
///
/// The restrictive API is intentional: signals receive an opaque `AsOfView`
/// instead of the event stream. That prevents mechanical lookahead through the
/// kernel surface, but it cannot detect out-of-band knowledge encoded by the
/// signal author or input data.
pub trait Signal {
    /// Evaluates signal state from the opaque as-of view.
    ///
    /// `as_of_timestamp` is the prediction event's timestamp for signal logic
    /// that needs to know the evaluation time. It is not the cutoff mechanism:
    /// the replay engine has already limited `view` by applying only events
    /// available at the prediction's replay key.
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
