#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Normalize,
    FeatureCompute,
    FeatureBatch,
    AlertRouting,
    Explain,
    RepairMalformedInput,
    GenerateHypothesis,
    BacktestSweep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    HotPathCpu,
    GpuBatch,
    AiSidecar,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workload {
    pub task: TaskKind,
    pub max_latency_ns: u64,
    pub deterministic_required: bool,
    pub blocks_hot_path: bool,
    pub human_consumed: bool,
    pub batch_size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementDecision {
    pub task: TaskKind,
    pub placement: Placement,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct PlacementPolicy;

impl PlacementPolicy {
    pub fn new() -> Self {
        Self
    }

    pub fn decide(&self, workload: &Workload) -> PlacementDecision {
        let (placement, reason) = if workload.blocks_hot_path && workload.deterministic_required {
            (
                Placement::HotPathCpu,
                "blocking deterministic work must remain on the hot path CPU",
            )
        } else if matches!(
            workload.task,
            TaskKind::FeatureBatch | TaskKind::BacktestSweep
        ) && workload.batch_size >= 32_768
            && !workload.blocks_hot_path
        {
            (
                Placement::GpuBatch,
                "large non-blocking batch is eligible for GPU throughput measurement",
            )
        } else if matches!(
            workload.task,
            TaskKind::Explain | TaskKind::RepairMalformedInput | TaskKind::GenerateHypothesis
        ) && workload.human_consumed
            && !workload.blocks_hot_path
        {
            (
                Placement::AiSidecar,
                "human-facing fuzzy work can run asynchronously outside ingest",
            )
        } else if !workload.deterministic_required && workload.blocks_hot_path {
            (
                Placement::Offline,
                "nondeterministic blocking work is not admissible in the hot path",
            )
        } else {
            (
                Placement::HotPathCpu,
                "default to deterministic CPU until evidence supports another placement",
            )
        };

        PlacementDecision {
            task: workload.task,
            placement,
            reason: reason.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_blocking_deterministic_work_on_cpu() {
        let policy = PlacementPolicy::new();
        let decision = policy.decide(&Workload {
            task: TaskKind::AlertRouting,
            max_latency_ns: 1_000_000,
            deterministic_required: true,
            blocks_hot_path: true,
            human_consumed: false,
            batch_size: 1,
        });

        assert_eq!(decision.placement, Placement::HotPathCpu);
    }

    #[test]
    fn routes_large_non_blocking_batches_to_gpu_candidate() {
        let policy = PlacementPolicy::new();
        let decision = policy.decide(&Workload {
            task: TaskKind::FeatureBatch,
            max_latency_ns: 10_000_000,
            deterministic_required: true,
            blocks_hot_path: false,
            human_consumed: false,
            batch_size: 100_000,
        });

        assert_eq!(decision.placement, Placement::GpuBatch);
    }

    #[test]
    fn routes_explanations_to_ai_sidecar() {
        let policy = PlacementPolicy::new();
        let decision = policy.decide(&Workload {
            task: TaskKind::Explain,
            max_latency_ns: 1_000_000_000,
            deterministic_required: false,
            blocks_hot_path: false,
            human_consumed: true,
            batch_size: 1,
        });

        assert_eq!(decision.placement, Placement::AiSidecar);
    }
}
