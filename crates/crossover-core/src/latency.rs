#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencySample {
    pub stage: &'static str,
    pub duration_ns: u128,
}

impl LatencySample {
    pub fn new(stage: &'static str, duration_ns: u128) -> Self {
        Self { stage, duration_ns }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencyReport {
    pub count: usize,
    pub min_ns: u128,
    pub p50_ns: u128,
    pub p95_ns: u128,
    pub p99_ns: u128,
    pub max_ns: u128,
}

impl LatencyReport {
    pub fn from_samples(samples: &[LatencySample]) -> Self {
        let mut durations: Vec<u128> = samples.iter().map(|sample| sample.duration_ns).collect();
        durations.sort_unstable();

        if durations.is_empty() {
            return Self {
                count: 0,
                min_ns: 0,
                p50_ns: 0,
                p95_ns: 0,
                p99_ns: 0,
                max_ns: 0,
            };
        }

        Self {
            count: durations.len(),
            min_ns: durations[0],
            p50_ns: percentile(&durations, 0.50),
            p95_ns: percentile(&durations, 0.95),
            p99_ns: percentile(&durations, 0.99),
            max_ns: *durations.last().unwrap_or(&0),
        }
    }

    pub fn format_summary(&self) -> String {
        format!(
            "samples={} min={}ns p50={}ns p95={}ns p99={}ns max={}ns",
            self.count, self.min_ns, self.p50_ns, self.p95_ns, self.p99_ns, self.max_ns
        )
    }
}

fn percentile(sorted: &[u128], percentile: f64) -> u128 {
    let index = ((sorted.len().saturating_sub(1)) as f64 * percentile).round() as usize;
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_latency_samples() {
        let samples = vec![
            LatencySample::new("x", 10),
            LatencySample::new("x", 20),
            LatencySample::new("x", 30),
            LatencySample::new("x", 40),
        ];

        let report = LatencyReport::from_samples(&samples);
        assert_eq!(report.count, 4);
        assert_eq!(report.min_ns, 10);
        assert_eq!(report.max_ns, 40);
    }
}
