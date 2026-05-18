use crate::{
    EventKind, FeatureEngine, FeatureError, FeatureUpdate, LatencySample, NormalizedEvent,
    PlacementDecision, PlacementPolicy, RawEvent, TaskKind, Workload,
};
use std::error::Error;
use std::fmt;
use std::time::Instant;

#[derive(Debug)]
pub struct Pipeline {
    feature_engine: FeatureEngine,
    policy: PlacementPolicy,
}

impl Pipeline {
    pub fn new() -> Self {
        Self {
            feature_engine: FeatureEngine::new(),
            policy: PlacementPolicy::new(),
        }
    }

    pub fn process(&mut self, raw: RawEvent) -> Result<PipelineOutput, PipelineError> {
        let normalize_start = Instant::now();
        let received_ns = raw.observed_ns;
        let normalized = raw.normalize(received_ns);
        let normalize_ns = normalize_start.elapsed().as_nanos();

        let feature_start = Instant::now();
        let feature_update = self.feature_engine.process(&normalized)?;
        let feature_ns = feature_start.elapsed().as_nanos();

        let policy_start = Instant::now();
        let decisions = self.decisions_for(&normalized);
        let policy_ns = policy_start.elapsed().as_nanos();

        Ok(PipelineOutput {
            normalized,
            feature_update,
            decisions,
            samples: vec![
                LatencySample::new("normalize", normalize_ns),
                LatencySample::new("feature", feature_ns),
                LatencySample::new("policy", policy_ns),
            ],
        })
    }

    fn decisions_for(&self, event: &NormalizedEvent) -> Vec<PlacementDecision> {
        let mut workloads = vec![
            Workload {
                task: TaskKind::Normalize,
                max_latency_ns: 250_000,
                deterministic_required: true,
                blocks_hot_path: true,
                human_consumed: false,
                batch_size: 1,
            },
            Workload {
                task: TaskKind::FeatureCompute,
                max_latency_ns: 1_000_000,
                deterministic_required: true,
                blocks_hot_path: true,
                human_consumed: false,
                batch_size: 1,
            },
        ];

        if matches!(event.kind, EventKind::News | EventKind::Filing | EventKind::AltData) {
            workloads.push(Workload {
                task: TaskKind::Explain,
                max_latency_ns: 2_000_000_000,
                deterministic_required: false,
                blocks_hot_path: false,
                human_consumed: true,
                batch_size: 1,
            });
        }

        workloads
            .iter()
            .map(|workload| self.policy.decide(workload))
            .collect()
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineOutput {
    pub normalized: NormalizedEvent,
    pub feature_update: Option<FeatureUpdate>,
    pub decisions: Vec<PlacementDecision>,
    pub samples: Vec<LatencySample>,
}

#[derive(Debug)]
pub enum PipelineError {
    Feature(FeatureError),
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Feature(source) => write!(f, "feature engine failed: {source}"),
        }
    }
}

impl Error for PipelineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Feature(source) => Some(source),
        }
    }
}

impl From<FeatureError> for PipelineError {
    fn from(source: FeatureError) -> Self {
        Self::Feature(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processes_tick_event() {
        let mut pipeline = Pipeline::new();
        let output = pipeline
            .process(RawEvent {
                source: "sim".to_string(),
                observed_ns: 1,
                sequence: 1,
                kind: EventKind::Tick,
                symbol: "AAPL".to_string(),
                payload: "price_cents=100,size=2".to_string(),
            })
            .unwrap();

        assert!(output.feature_update.is_some());
        assert_eq!(output.decisions.len(), 2);
    }

    #[test]
    fn routes_news_explanation_to_sidecar() {
        let mut pipeline = Pipeline::new();
        let output = pipeline
            .process(RawEvent {
                source: "sim".to_string(),
                observed_ns: 1,
                sequence: 1,
                kind: EventKind::News,
                symbol: "AAPL".to_string(),
                payload: "headline=test".to_string(),
            })
            .unwrap();

        assert_eq!(output.feature_update, None);
        assert_eq!(output.decisions.len(), 3);
    }
}
