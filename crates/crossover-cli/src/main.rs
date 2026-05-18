use crossover_core::{
    parse_pipe_events, EventKind, LatencyReport, LatencySample, Pipeline, RawEvent,
};
use std::env;
use std::error::Error;
use std::fs;
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
}
