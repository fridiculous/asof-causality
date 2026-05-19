use crossover_core::{
    parse_pipe_events, EventKind, LatencyReport, LatencySample, Pipeline, RawEvent,
};
use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Instant;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("replay") => replay(
            args.get(2)
                .map(String::as_str)
                .unwrap_or("examples/events.pipe"),
        ),
        Some("bench") => {
            let count = args
                .get(2)
                .map(String::as_str)
                .unwrap_or("100000")
                .parse::<usize>()?;
            bench(count)
        }
        Some("generate-scenario") => {
            let input = args.get(2).ok_or_else(|| missing_arg("input path"))?;
            let output = args.get(3).ok_or_else(|| missing_arg("output path"))?;
            let profile = args.get(4).map(String::as_str).unwrap_or("messy");
            let event_count = args
                .get(5)
                .map(String::as_str)
                .unwrap_or("10000")
                .parse::<usize>()?;
            let seed = args
                .get(6)
                .map(String::as_str)
                .unwrap_or("1")
                .parse::<u64>()?;

            generate_scenario(input, output, profile, event_count, seed)
        }
        _ => {
            print_help();
            Ok(())
        }
    }
}

fn replay(path: &str) -> Result<(), Box<dyn Error>> {
    let input = fs::read_to_string(path)?;
    let events = parse_pipe_events(&input)?;
    let mut pipeline = Pipeline::new();
    let mut samples = Vec::new();

    println!("replay path={path} events={}", events.len());

    for event in events {
        let output = pipeline.process(event)?;
        samples.extend(output.samples);

        println!(
            "event seq={} kind={} symbol={}",
            output.normalized.sequence,
            output.normalized.kind.as_str(),
            output.normalized.symbol
        );

        if let Some(update) = output.feature_update {
            println!(
                "  feature price_cents={} size={} previous={:?} return_bps={:?} cumulative_size={}",
                update.price_cents,
                update.size,
                update.previous_price_cents,
                update.return_bps,
                update.cumulative_size
            );
        }

        for decision in output.decisions {
            println!(
                "  placement task={:?} placement={:?} reason={}",
                decision.task, decision.placement, decision.reason
            );
        }
    }

    let report = LatencyReport::from_samples(&samples);
    println!("latency {}", report.format_summary());
    Ok(())
}

fn bench(count: usize) -> Result<(), Box<dyn Error>> {
    let mut pipeline = Pipeline::new();
    let mut samples: Vec<LatencySample> = Vec::with_capacity(count.saturating_mul(3));
    let start = Instant::now();

    for index in 0..count {
        let output = pipeline.process(synthetic_tick(index))?;
        samples.extend(output.samples);
    }

    let elapsed = start.elapsed();
    let seconds = elapsed.as_secs_f64();
    let events_per_second = if seconds > 0.0 {
        count as f64 / seconds
    } else {
        0.0
    };
    let report = LatencyReport::from_samples(&samples);

    println!("bench events={count} elapsed_ms={:.3}", seconds * 1000.0);
    println!("throughput events_per_second={events_per_second:.0}");
    println!("latency {}", report.format_summary());
    Ok(())
}

fn generate_scenario(
    input_path: &str,
    output_path: &str,
    profile_name: &str,
    event_count: usize,
    seed: u64,
) -> Result<(), Box<dyn Error>> {
    let profile = ScenarioProfile::parse(profile_name)?;
    let input = fs::read_to_string(input_path)?;
    let parsed_events = parse_pipe_events(&input)?;
    let source_events = expand_seed_events(&parsed_events, event_count);

    let mut rng = DeterministicRng::new(seed);
    let mut generated = Vec::with_capacity(profile.output_capacity(source_events.len()));
    let mut stale_source_sequences = [0_u64; STALE_SOURCES.len()];
    let gap_start = event_count / 3;
    let stale_start = event_count / 3;
    let stale_end = stale_start.saturating_add(event_count / 5);

    for (index, event) in source_events.into_iter().enumerate() {
        if should_drop_event(profile, index, event_count, &mut rng) {
            continue;
        }

        let (source_time_ns, source_clock_skewed) =
            source_time_for(profile, event.observed_ns, &mut rng);
        let ingest_time_ns = ingest_time_for(
            profile,
            index,
            event_count,
            event.observed_ns,
            source_time_ns,
            source_clock_skewed,
            &mut rng,
        );
        let mut primary = event;

        if profile == ScenarioProfile::StaleSource {
            let source_index = index % STALE_SOURCES.len();
            primary.source = STALE_SOURCES[source_index].to_string();
            stale_source_sequences[source_index] =
                stale_source_sequences[source_index].saturating_add(1);
            primary.sequence = stale_source_sequences[source_index];
        }

        if profile == ScenarioProfile::SequenceGap && index >= gap_start {
            primary.sequence = primary.sequence.saturating_add(SEQUENCE_GAP_SIZE);
        }

        primary.observed_ns = ingest_time_ns;
        let mut fields = vec![
            ("scenario", profile.as_str().to_string()),
            ("source_time_ns", source_time_ns.to_string()),
            ("ingest_time_ns", ingest_time_ns.to_string()),
            (
                "latency_ns",
                ingest_time_ns.saturating_sub(source_time_ns).to_string(),
            ),
        ];

        if source_clock_skewed {
            fields.push(("clock_skew", "forward_source_clock".to_string()));
            fields.push((
                "source_ahead_ns",
                source_time_ns.saturating_sub(ingest_time_ns).to_string(),
            ));
        }

        if profile == ScenarioProfile::CapacityExhaustion {
            fields.push(("capacity_probe", "true".to_string()));
        }

        if is_out_of_order_burst_event(profile, index, event_count) {
            fields.push(("out_of_order_burst", "true".to_string()));
        }

        if is_late_arrival_event(profile, index) {
            fields.push(("late_arrival", "true".to_string()));
        }

        if profile == ScenarioProfile::SequenceGap && index == gap_start {
            fields.push((
                "sequence_gap_after",
                primary
                    .sequence
                    .saturating_sub(SEQUENCE_GAP_SIZE)
                    .saturating_sub(1)
                    .to_string(),
            ));
            fields.push(("sequence_gap_size", SEQUENCE_GAP_SIZE.to_string()));
        }

        if profile == ScenarioProfile::StaleSource && index >= stale_start && index < stale_end {
            fields.push((
                "stale_source",
                STALE_SOURCES[STALE_SOURCE_INDEX].to_string(),
            ));
            fields.push(("quorum", "partial".to_string()));
        }

        primary.payload = append_payload_fields(&primary.payload, &fields);

        if should_duplicate_event(profile, &mut rng) {
            let mut duplicate = primary.clone();
            duplicate.observed_ns = duplicate
                .observed_ns
                .saturating_add(1_000_000 + rng.range(250_000_000));
            duplicate.payload =
                upsert_payload_field(&duplicate.payload, "ingest_time_ns", duplicate.observed_ns);
            duplicate.payload = upsert_payload_field(
                &duplicate.payload,
                "latency_ns",
                duplicate.observed_ns.saturating_sub(source_time_ns),
            );
            duplicate.payload = append_payload_fields(
                &duplicate.payload,
                &[("duplicate_of", primary.sequence.to_string())],
            );
            generated.push(duplicate);
        }

        if should_correct_event(profile, index, &mut rng) {
            let mut correction = primary.clone();
            correction.sequence = correction
                .sequence
                .saturating_add(10_000_000_000)
                .saturating_add(index as u64);
            correction.observed_ns = correction
                .observed_ns
                .saturating_add(2_000_000_000 + rng.range(60_000_000_000));
            correction.payload = upsert_payload_field(
                &correction.payload,
                "ingest_time_ns",
                correction.observed_ns,
            );
            correction.payload = upsert_payload_field(
                &correction.payload,
                "latency_ns",
                correction.observed_ns.saturating_sub(source_time_ns),
            );
            correction.payload =
                adjust_price_cents(&correction.payload, correction_delta_bps(&mut rng));
            correction.payload = append_payload_fields(
                &correction.payload,
                &[
                    ("correction_for", primary.sequence.to_string()),
                    ("correction_ingest_ns", correction.observed_ns.to_string()),
                ],
            );
            generated.push(correction);
        }

        generated.push(primary);
    }

    generated.sort_by_key(|event| (event.observed_ns, event.sequence));

    if let Some(parent) = Path::new(output_path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let mut output = String::new();
    writeln!(
        &mut output,
        "# source|observed_ns|sequence|kind|symbol|payload"
    )
    .expect("writing to String cannot fail");
    writeln!(
        &mut output,
        "# generated profile={} seed={} requested_events={} output_events={}",
        profile.as_str(),
        seed,
        event_count,
        generated.len()
    )
    .expect("writing to String cannot fail");

    for event in &generated {
        writeln!(
            &mut output,
            "{}|{}|{}|{}|{}|{}",
            event.source,
            event.observed_ns,
            event.sequence,
            event.kind.as_str(),
            event.symbol,
            event.payload
        )
        .expect("writing to String cannot fail");
    }

    fs::write(output_path, output)?;
    println!(
        "generated scenario profile={} seed={} input_events={} output_events={} path={}",
        profile.as_str(),
        seed,
        event_count,
        generated.len(),
        output_path
    );
    Ok(())
}

fn synthetic_tick(index: usize) -> RawEvent {
    let symbol = if index % 2 == 0 { "AAPL" } else { "MSFT" };
    let price_cents = 10_000 + (index % 1_000) as i64;
    let size = 1 + (index % 100) as u64;

    RawEvent {
        source: "synthetic".to_string(),
        observed_ns: index as u64,
        sequence: index as u64,
        kind: EventKind::Tick,
        symbol: symbol.to_string(),
        payload: format!("price_cents={price_cents},size={size}"),
    }
}

fn print_help() {
    println!("crossover");
    println!();
    println!("usage:");
    println!("  crossover replay [path]");
    println!("  crossover bench [event_count]");
    println!(
        "  crossover generate-scenario <input.pipe> <output.pipe> [profile] [event_count] [seed]"
    );
    println!("profiles:");
    println!("  ordered, messy, adversarial");
    println!("  capacity_exhaustion, out_of_order_burst, sequence_gap");
    println!("  late_arrival, correction, clock_skew, stale_source");
}

fn missing_arg(name: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("missing {name}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScenarioProfile {
    Ordered,
    Messy,
    Adversarial,
    CapacityExhaustion,
    OutOfOrderBurst,
    SequenceGap,
    LateArrival,
    Correction,
    ClockSkew,
    StaleSource,
}

impl ScenarioProfile {
    fn parse(value: &str) -> Result<Self, io::Error> {
        match value {
            "ordered" => Ok(Self::Ordered),
            "messy" => Ok(Self::Messy),
            "adversarial" => Ok(Self::Adversarial),
            "capacity" | "capacity_exhaustion" | "capacity-exhaustion" => {
                Ok(Self::CapacityExhaustion)
            }
            "out_of_order" | "out_of_order_burst" | "out-of-order-burst" => {
                Ok(Self::OutOfOrderBurst)
            }
            "sequence_gap" | "sequence-gap" => Ok(Self::SequenceGap),
            "late" | "late_arrival" | "late-arrival" => Ok(Self::LateArrival),
            "correction" | "corrections" => Ok(Self::Correction),
            "clock_skew" | "clock-skew" => Ok(Self::ClockSkew),
            "stale" | "stale_source" | "stale-source" => Ok(Self::StaleSource),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown scenario profile: {other}"),
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Ordered => "ordered",
            Self::Messy => "messy",
            Self::Adversarial => "adversarial",
            Self::CapacityExhaustion => "capacity_exhaustion",
            Self::OutOfOrderBurst => "out_of_order_burst",
            Self::SequenceGap => "sequence_gap",
            Self::LateArrival => "late_arrival",
            Self::Correction => "correction",
            Self::ClockSkew => "clock_skew",
            Self::StaleSource => "stale_source",
        }
    }

    fn output_capacity(self, input_events: usize) -> usize {
        match self {
            Self::Ordered
            | Self::CapacityExhaustion
            | Self::OutOfOrderBurst
            | Self::SequenceGap
            | Self::LateArrival
            | Self::ClockSkew
            | Self::StaleSource => input_events,
            Self::Messy => input_events + input_events / 20,
            Self::Adversarial | Self::Correction => input_events + input_events / 5,
        }
    }
}

const SEQUENCE_GAP_SIZE: u64 = 1_000;
const STALE_SOURCE_INDEX: usize = 1;
const STALE_SOURCES: [&str; 3] = ["binance_sim", "coinbase_sim", "kraken_sim"];

fn expand_seed_events(seed_events: &[RawEvent], event_count: usize) -> Vec<RawEvent> {
    if seed_events.is_empty() || event_count == 0 {
        return Vec::new();
    }

    let first_observed_ns = seed_events
        .first()
        .map(|event| event.observed_ns)
        .unwrap_or_default();
    let last_observed_ns = seed_events
        .last()
        .map(|event| event.observed_ns)
        .unwrap_or(first_observed_ns);
    let cycle_stride_ns = last_observed_ns
        .saturating_sub(first_observed_ns)
        .max(1_000_000_000)
        .saturating_add(1_000_000);
    let cycle_stride_sequence = 1_000_000_000_000_u64;

    let mut expanded = Vec::with_capacity(event_count);
    for index in 0..event_count {
        let cycle = index / seed_events.len();
        let mut event = seed_events[index % seed_events.len()].clone();
        event.observed_ns = event
            .observed_ns
            .saturating_add((cycle as u64).saturating_mul(cycle_stride_ns));
        event.sequence = event
            .sequence
            .saturating_add((cycle as u64).saturating_mul(cycle_stride_sequence));

        if cycle > 0 {
            event.payload =
                append_payload_fields(&event.payload, &[("seed_cycle", cycle.to_string())]);
        }

        expanded.push(event);
    }

    expanded
}

fn should_drop_event(
    profile: ScenarioProfile,
    index: usize,
    event_count: usize,
    rng: &mut DeterministicRng,
) -> bool {
    match profile {
        ScenarioProfile::Adversarial => rng.chance_per_million(20_000),
        ScenarioProfile::StaleSource => {
            let stale_start = event_count / 3;
            let stale_end = stale_start.saturating_add(event_count / 5);
            index % STALE_SOURCES.len() == STALE_SOURCE_INDEX
                && index >= stale_start
                && index < stale_end
        }
        _ => false,
    }
}

fn should_duplicate_event(profile: ScenarioProfile, rng: &mut DeterministicRng) -> bool {
    matches!(
        profile,
        ScenarioProfile::Messy | ScenarioProfile::Adversarial
    ) && rng.chance_per_million(30_000)
}

fn should_correct_event(
    profile: ScenarioProfile,
    index: usize,
    rng: &mut DeterministicRng,
) -> bool {
    match profile {
        ScenarioProfile::Correction => index % 25 == 5,
        ScenarioProfile::Adversarial => rng.chance_per_million(20_000),
        _ => false,
    }
}

fn is_out_of_order_burst_event(profile: ScenarioProfile, index: usize, event_count: usize) -> bool {
    if profile != ScenarioProfile::OutOfOrderBurst {
        return false;
    }

    let burst_start = event_count / 3;
    let burst_end = burst_start.saturating_add(event_count / 5);
    index >= burst_start && index < burst_end
}

fn is_late_arrival_event(profile: ScenarioProfile, index: usize) -> bool {
    profile == ScenarioProfile::LateArrival && index % 100 == 10
}

fn source_time_for(
    profile: ScenarioProfile,
    observed_ns: u64,
    rng: &mut DeterministicRng,
) -> (u64, bool) {
    match profile {
        ScenarioProfile::ClockSkew if rng.chance_per_million(200_000) => {
            (observed_ns.saturating_add(50_000_000), true)
        }
        ScenarioProfile::Adversarial if rng.chance_per_million(15_000) => (
            observed_ns.saturating_add(50_000_000 + rng.range(100_000_000)),
            true,
        ),
        _ => (observed_ns, false),
    }
}

fn ingest_time_for(
    profile: ScenarioProfile,
    index: usize,
    event_count: usize,
    observed_ns: u64,
    source_time_ns: u64,
    source_clock_skewed: bool,
    rng: &mut DeterministicRng,
) -> u64 {
    if source_clock_skewed {
        observed_ns.saturating_add(100_000 + rng.range(200_000))
    } else {
        source_time_ns.saturating_add(latency_ns_for(profile, index, event_count, rng))
    }
}

fn latency_ns_for(
    profile: ScenarioProfile,
    index: usize,
    event_count: usize,
    rng: &mut DeterministicRng,
) -> u64 {
    let roll = rng.range(1_000_000);
    match profile {
        ScenarioProfile::Ordered
        | ScenarioProfile::CapacityExhaustion
        | ScenarioProfile::SequenceGap
        | ScenarioProfile::Correction
        | ScenarioProfile::ClockSkew
        | ScenarioProfile::StaleSource => 100_000 + rng.range(500_000),
        ScenarioProfile::Messy => {
            if roll < 950_000 {
                rng.range(10_000_000)
            } else if roll < 999_000 {
                10_000_000 + rng.range(1_990_000_000)
            } else {
                2_000_000_000 + rng.range(58_000_000_000)
            }
        }
        ScenarioProfile::Adversarial => {
            if roll < 700_000 {
                rng.range(5_000_000)
            } else if roll < 950_000 {
                5_000_000 + rng.range(2_000_000_000)
            } else {
                2_000_000_000 + rng.range(120_000_000_000)
            }
        }
        ScenarioProfile::OutOfOrderBurst => {
            if is_out_of_order_burst_event(profile, index, event_count) {
                2_000_000_000 + rng.range(30_000_000_000)
            } else {
                rng.range(5_000_000)
            }
        }
        ScenarioProfile::LateArrival => {
            if is_late_arrival_event(profile, index) {
                60_000_000_000 + rng.range(60_000_000_000)
            } else {
                rng.range(5_000_000)
            }
        }
    }
}

fn correction_delta_bps(rng: &mut DeterministicRng) -> i64 {
    let magnitude = 1 + rng.range(50) as i64;
    if rng.chance_per_million(500_000) {
        magnitude
    } else {
        -magnitude
    }
}

fn append_payload_fields(payload: &str, fields: &[(&str, String)]) -> String {
    let extra_len = fields
        .iter()
        .map(|(key, value)| key.len() + value.len() + 2)
        .sum::<usize>();
    let mut output = String::with_capacity(payload.len() + extra_len);
    output.push_str(payload);

    for (key, value) in fields {
        if !output.is_empty() {
            output.push(',');
        }
        output.push_str(key);
        output.push('=');
        output.push_str(value);
    }

    output
}

fn upsert_payload_field(payload: &str, key: &str, value: u64) -> String {
    let key_prefix = format!("{key}=");
    let value = value.to_string();
    let mut output = String::with_capacity(payload.len() + key.len() + value.len() + 2);
    let mut replaced = false;

    for (index, pair) in payload.split(',').enumerate() {
        if index > 0 {
            output.push(',');
        }

        if pair.starts_with(&key_prefix) {
            output.push_str(&key_prefix);
            output.push_str(&value);
            replaced = true;
        } else {
            output.push_str(pair);
        }
    }

    if !replaced {
        if !output.is_empty() {
            output.push(',');
        }
        output.push_str(&key_prefix);
        output.push_str(&value);
    }

    output
}

fn adjust_price_cents(payload: &str, delta_bps: i64) -> String {
    let mut output = String::with_capacity(payload.len() + 8);

    for (index, pair) in payload.split(',').enumerate() {
        if index > 0 {
            output.push(',');
        }

        if let Some(value) = pair.strip_prefix("price_cents=") {
            if let Ok(price_cents) = value.parse::<i64>() {
                let delta = price_cents.saturating_mul(delta_bps) / 10_000;
                output.push_str("price_cents=");
                output.push_str(&price_cents.saturating_add(delta).to_string());
                continue;
            }
        }

        output.push_str(pair);
    }

    output
}

struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }

    fn range(&mut self, upper_exclusive: u64) -> u64 {
        if upper_exclusive == 0 {
            0
        } else {
            self.next() % upper_exclusive
        }
    }

    fn chance_per_million(&mut self, chance: u32) -> bool {
        self.range(1_000_000) < chance as u64
    }
}
