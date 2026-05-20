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
    run_adversarial_checks, run_adversarial_checks_with_options,
    run_adversarial_checks_with_options_for_signal, CheckOptions, CheckReport, CheckResult,
};
pub use event::{Event, EventRole, ParseEventError, Sentiment};
pub use generator::{generate_events, GenerateConfig, GeneratedStream, GenerationStats, Scenario};
pub use ids::{EventKey, InputSet, SymbolId, MAX_INPUTS_PER_PREDICTION};
pub use log::{
    blake3_digest, blake3_hex, feature_recipe_hash, fnv1a64, hex_digest, FeatureRecipeHash,
    PredictionLog, PredictionRecord,
};
pub use replay::{
    parse_pipe_events, ReplayEngine, ReplayError, ReplayOptions, ReplayOrder, ReplayOutput,
};
pub use signal::{LastFeatureSentimentSignal, Signal, WindowedFeatureSentimentSignal};
pub use state::{AsOfView, SymbolSnapshot};
