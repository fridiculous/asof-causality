//! Deterministic as-of replay primitives for auditing temporal leakage.
//!
//! The crate parses two-clock event streams, replays them in received-time
//! order, exposes only an opaque [`AsOfView`] to signal code, and emits
//! [`PredictionRecord`] values with input provenance so callers can verify that
//! each prediction used only data available at its replay key.

#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

#[allow(missing_docs)]
pub mod bench;
pub(crate) mod catalog;
#[allow(missing_docs)]
pub mod checks;
/// Event model and pipe-record parsing.
pub mod event;
#[allow(missing_docs)]
pub mod generator;
/// Stable compact identifiers and fixed-capacity provenance sets.
pub mod ids;
/// Prediction records, transcripts, and digest helpers.
pub mod log;
/// Received-time replay engine.
pub mod replay;
/// Signal trait and built-in signals.
pub mod signal;
/// Opaque as-of state view exposed to signals.
pub mod state;

pub use bench::{run_representation_benchmark, BenchResult};
pub use checks::{
    run_adversarial_checks, run_adversarial_checks_with_options,
    run_adversarial_checks_with_options_for_signal, CheckOptions, CheckReport, CheckResult,
};
pub use event::{
    Event, EventRole, FeatureDType, FeatureSpec, FixedDecimal, ParseEventError, Sentiment,
    BUILTIN_FEATURE_SPECS, FIXED_DECIMAL_SCALE, FIXED_DECIMAL_SCALE_DIGITS, SCORE_FEATURE,
    SENTIMENT_FEATURE,
};
pub use generator::{generate_events, GenerateConfig, GeneratedStream, GenerationStats, Scenario};
pub use ids::{EventKey, InputSet, SymbolId, SymbolSlot, MAX_INPUTS_PER_PREDICTION};
pub use log::{
    blake3_digest, blake3_hex, feature_recipe_hash, fnv1a64, hex_digest, FeatureRecipeHash,
    PredictionLog, PredictionRecord,
};
pub use replay::{
    parse_pipe_events, ReplayEngine, ReplayError, ReplayOptions, ReplayOrder, ReplayOutput,
};
pub use signal::{
    LastFeatureSentimentSignal, Signal, VolAdjustedMomentumSignal, WindowedFeatureSentimentSignal,
    WindowedZScoreSignal,
};
pub use state::{AsOfView, SymbolSnapshot};
