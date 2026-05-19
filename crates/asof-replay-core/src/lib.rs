pub mod bench;
pub mod checks;
pub mod event;
pub mod generator;
pub mod ids;
pub mod log;
pub mod replay;
pub mod signal;
pub mod state;

pub use bench::{run_representation_benchmark, BenchResult};
pub use checks::{
    run_adversarial_checks, run_adversarial_checks_with_options, run_universal_leakage_checks,
    run_universal_leakage_checks_with_options, CheckOptions, CheckReport, CheckResult,
};
pub use event::{Event, EventKind, ParseEventError, Sentiment};
pub use generator::{generate_events, GenerateConfig, GeneratedStream, GenerationStats, Scenario};
pub use ids::{EventKey, InputSet};
pub use log::{PredictionLog, PredictionRecord};
pub use replay::{
    parse_pipe_events, ReplayEngine, ReplayError, ReplayOptions, ReplayOrder, ReplayOutput,
};
pub use signal::{LastSentimentSignal, Signal};
pub use state::{AsOfView, SymbolSnapshot};
