use crate::{Event, EventRole};
use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    Clean,
    LateHeavy,
    FeatureCorrectionHeavy,
}

impl Scenario {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "clean" => Some(Self::Clean),
            "late-heavy" => Some(Self::LateHeavy),
            "feature-correction-heavy" | "correction-heavy" => Some(Self::FeatureCorrectionHeavy),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::LateHeavy => "late-heavy",
            Self::FeatureCorrectionHeavy => "feature-correction-heavy",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenerateConfig {
    pub scenario: Scenario,
    pub events: usize,
    pub symbols: usize,
    pub late_rate: f64,
    pub feature_correction_rate: f64,
    pub outcome_rate: f64,
    pub prediction_interval: usize,
    pub max_lag: u64,
    pub seed: u64,
    pub shuffle_physical_order: bool,
}

impl GenerateConfig {
    pub fn for_scenario(scenario: Scenario) -> Self {
        match scenario {
            Scenario::Clean => Self {
                scenario,
                events: 10_000,
                symbols: 128,
                late_rate: 0.0,
                feature_correction_rate: 0.0,
                outcome_rate: 0.01,
                prediction_interval: 10,
                max_lag: 100,
                seed: 42,
                shuffle_physical_order: false,
            },
            Scenario::LateHeavy => Self {
                scenario,
                events: 100_000,
                symbols: 1_024,
                late_rate: 0.30,
                feature_correction_rate: 0.05,
                outcome_rate: 0.01,
                prediction_interval: 10,
                max_lag: 300,
                seed: 42,
                shuffle_physical_order: true,
            },
            Scenario::FeatureCorrectionHeavy => Self {
                scenario,
                events: 100_000,
                symbols: 1_024,
                late_rate: 0.10,
                feature_correction_rate: 0.25,
                outcome_rate: 0.01,
                prediction_interval: 10,
                max_lag: 300,
                seed: 42,
                shuffle_physical_order: true,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationStats {
    pub scenario: Scenario,
    pub seed: u64,
    pub data_events: usize,
    pub rows: usize,
    pub symbols: usize,
    pub features: usize,
    pub predictions: usize,
    pub feature_corrections: usize,
    pub outcomes: usize,
    pub late_updates: usize,
    pub shuffled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedStream {
    pub events: Vec<Event>,
    pub stats: GenerationStats,
}

impl GeneratedStream {
    pub fn to_pipe_string(&self) -> String {
        let mut output = String::new();
        let _ = writeln!(output, "# generated_by=asof-causality");
        let _ = writeln!(
            output,
            "# scenario={} seed={} data_events={} rows={} symbols={} shuffled={}",
            self.stats.scenario.as_str(),
            self.stats.seed,
            self.stats.data_events,
            self.stats.rows,
            self.stats.symbols,
            self.stats.shuffled
        );
        let _ = writeln!(
            output,
            "# event_id|observed_time|received_time|received_sequence_number|role|symbol|payload"
        );

        for event in &self.events {
            let _ = writeln!(output, "{}", event.to_pipe_record());
        }

        output
    }
}

pub fn generate_events(config: &GenerateConfig) -> GeneratedStream {
    let symbols = config.symbols.max(1);
    let data_events = config.events.max(1);
    let prediction_interval = config.prediction_interval.max(1);
    let max_lag = config.max_lag.max(1);
    let late_rate = clamp_rate(config.late_rate);
    let feature_correction_rate = clamp_rate(config.feature_correction_rate);
    let outcome_rate = clamp_rate(config.outcome_rate);

    let mut rng = SplitMix64::new(config.seed);
    let mut events = Vec::with_capacity(data_events + data_events / prediction_interval + 8);
    let mut received_sequence_number = 1_u64;

    add_sentinel_events(
        &mut events,
        &mut received_sequence_number,
        feature_correction_rate > 0.0
            || matches!(config.scenario, Scenario::FeatureCorrectionHeavy),
    );

    let mut stats = GenerationStats {
        scenario: config.scenario,
        seed: config.seed,
        data_events,
        rows: 0,
        symbols,
        features: 1,
        predictions: 3,
        feature_corrections: 0,
        outcomes: 0,
        late_updates: 1,
        shuffled: config.shuffle_physical_order,
    };

    if feature_correction_rate > 0.0 || matches!(config.scenario, Scenario::FeatureCorrectionHeavy)
    {
        stats.feature_corrections += 1;
        stats.predictions += 2;
        stats.late_updates += 1;
    }

    for index in 0..data_events {
        let observed_time = 2_000 + index as u64 * 10;
        let symbol_index = rng.range_usize(symbols);
        let symbol = symbol_name(symbol_index);
        let is_late = rng.chance(late_rate);
        let lag = if is_late {
            2 + rng.range_u64(max_lag)
        } else {
            0
        };
        let received_time = observed_time + lag;
        let sentiment = sentiment_payload(rng.range_usize(3));

        events.push(Event::new(
            format!("n{index}"),
            observed_time,
            received_time,
            next_received_sequence_number(&mut received_sequence_number),
            EventRole::Feature,
            symbol.clone(),
            format!("sentiment={sentiment}"),
        ));
        stats.features += 1;
        if is_late {
            stats.late_updates += 1;
        }

        if index % prediction_interval == 0 {
            events.push(Event::new(
                format!("p{index}"),
                observed_time + 1,
                observed_time + 1,
                next_received_sequence_number(&mut received_sequence_number),
                EventRole::Prediction,
                symbol.clone(),
                "",
            ));
            stats.predictions += 1;
        }

        if rng.chance(feature_correction_rate) {
            let correction_time = received_time + 1 + rng.range_u64(max_lag);
            let correction_sentiment = sentiment_payload(rng.range_usize(3));
            events.push(Event::new(
                format!("c{index}"),
                observed_time + 2,
                correction_time,
                next_received_sequence_number(&mut received_sequence_number),
                EventRole::FeatureCorrection,
                symbol.clone(),
                format!("sentiment={correction_sentiment},corrects=n{index}"),
            ));
            stats.feature_corrections += 1;
            if correction_time > observed_time + 2 {
                stats.late_updates += 1;
            }
        }

        if rng.chance(outcome_rate) {
            events.push(Event::new(
                format!("l{index}"),
                observed_time + max_lag + 1,
                observed_time + max_lag + 1,
                next_received_sequence_number(&mut received_sequence_number),
                EventRole::Outcome,
                symbol,
                format!("return_bps={}", rng.range_i32(401) - 200),
            ));
            stats.outcomes += 1;
        }
    }

    if config.shuffle_physical_order {
        shuffle(&mut events, &mut rng);
    }

    stats.rows = events.len();
    GeneratedStream { events, stats }
}

fn add_sentinel_events(
    events: &mut Vec<Event>,
    received_sequence_number: &mut u64,
    include_correction: bool,
) {
    let symbol = symbol_name(0);
    events.push(Event::new(
        "p_sentinel_before",
        1_000,
        1_000,
        next_received_sequence_number(received_sequence_number),
        EventRole::Prediction,
        symbol.clone(),
        "",
    ));
    events.push(Event::new(
        "n_sentinel_late",
        1_000,
        1_020,
        next_received_sequence_number(received_sequence_number),
        EventRole::Feature,
        symbol.clone(),
        "sentiment=positive",
    ));
    events.push(Event::new(
        "p_sentinel_between",
        1_010,
        1_010,
        next_received_sequence_number(received_sequence_number),
        EventRole::Prediction,
        symbol.clone(),
        "",
    ));
    events.push(Event::new(
        "p_sentinel_after",
        1_030,
        1_030,
        next_received_sequence_number(received_sequence_number),
        EventRole::Prediction,
        symbol.clone(),
        "",
    ));

    if include_correction {
        events.push(Event::new(
            "c_sentinel_late",
            1_040,
            1_060,
            next_received_sequence_number(received_sequence_number),
            EventRole::FeatureCorrection,
            symbol.clone(),
            "sentiment=negative,corrects=n_sentinel_late",
        ));
        events.push(Event::new(
            "p_sentinel_before_correction",
            1_050,
            1_050,
            next_received_sequence_number(received_sequence_number),
            EventRole::Prediction,
            symbol.clone(),
            "",
        ));
        events.push(Event::new(
            "p_sentinel_after_correction",
            1_070,
            1_070,
            next_received_sequence_number(received_sequence_number),
            EventRole::Prediction,
            symbol,
            "",
        ));
    }
}

fn symbol_name(index: usize) -> String {
    format!("SYM{index:04}")
}

fn sentiment_payload(index: usize) -> &'static str {
    match index % 3 {
        0 => "negative",
        1 => "neutral",
        _ => "positive",
    }
}

fn next_received_sequence_number(received_sequence_number: &mut u64) -> u64 {
    let value = *received_sequence_number;
    *received_sequence_number += 1;
    value
}

fn clamp_rate(rate: f64) -> f64 {
    if rate.is_nan() {
        0.0
    } else {
        rate.clamp(0.0, 1.0)
    }
}

fn shuffle(events: &mut [Event], rng: &mut SplitMix64) {
    for index in (1..events.len()).rev() {
        let swap_index = rng.range_usize(index + 1);
        events.swap(index, swap_index);
    }
}

#[derive(Debug, Clone, Copy)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    }

    fn range_u64(&mut self, upper_exclusive: u64) -> u64 {
        if upper_exclusive == 0 {
            0
        } else {
            self.next_u64() % upper_exclusive
        }
    }

    fn range_usize(&mut self, upper_exclusive: usize) -> usize {
        self.range_u64(upper_exclusive as u64) as usize
    }

    fn range_i32(&mut self, upper_exclusive: i32) -> i32 {
        self.range_u64(upper_exclusive as u64) as i32
    }

    fn chance(&mut self, probability: f64) -> bool {
        if probability <= 0.0 || probability.is_nan() {
            return false;
        }
        if probability >= 1.0 {
            return true;
        }

        let threshold = (probability * u64::MAX as f64) as u64;
        self.next_u64() <= threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_pipe_events, run_adversarial_checks, ReplayEngine, ReplayOptions};

    #[test]
    fn same_seed_produces_same_pipe_file() {
        let config = GenerateConfig {
            events: 128,
            symbols: 8,
            ..GenerateConfig::for_scenario(Scenario::LateHeavy)
        };

        let left = generate_events(&config).to_pipe_string();
        let right = generate_events(&config).to_pipe_string();

        assert_eq!(left, right);
    }

    #[test]
    fn generated_fixture_round_trips_and_passes_checks() {
        let config = GenerateConfig {
            events: 512,
            symbols: 16,
            seed: 7,
            ..GenerateConfig::for_scenario(Scenario::LateHeavy)
        };
        let generated = generate_events(&config);
        let parsed = parse_pipe_events(&generated.to_pipe_string()).unwrap();

        let output = ReplayEngine::new()
            .replay(&parsed, ReplayOptions::default())
            .unwrap();
        assert!(!output.predictions.records().is_empty());

        let report = run_adversarial_checks(&parsed);
        assert!(report.passed(), "{report:?}");
    }

    #[test]
    fn chance_handles_probability_endpoints() {
        let mut rng = SplitMix64::new(0);

        for _ in 0..1024 {
            assert!(!rng.chance(0.0));
            assert!(!rng.chance(f64::NAN));
            assert!(rng.chance(1.0));
        }
    }
}
