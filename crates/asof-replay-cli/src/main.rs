use asof_replay_core::{
    generate_events, parse_pipe_events, run_adversarial_checks_with_options,
    run_representation_benchmark, CheckOptions, CheckReport, GenerateConfig, GeneratedStream,
    PredictionRecord, ReplayEngine, ReplayOptions, ReplayOrder, ReplayOutput, Scenario,
};
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

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
                .unwrap_or("examples/late-arrival.pipe"),
        ),
        Some("check") => check(&args[2..]),
        Some("compare-leaky") => compare_leaky(
            args.get(2)
                .map(String::as_str)
                .unwrap_or("examples/lookahead-negative-control.pipe"),
        ),
        Some("generate") => generate(&args[2..]),
        Some("run-suite") => run_suite(&args[2..]),
        Some("bench") => bench(&args[2..]),
        _ => {
            print_help();
            Ok(())
        }
    }
}

fn replay(path: &str) -> Result<(), Box<dyn Error>> {
    let events = load_events(path)?;
    let output = ReplayEngine::new().replay(&events, ReplayOptions::default())?;

    println!("replay path={path} events={}", output.replayed_events);
    println!(
        "prediction_time|prediction_sequence|symbol|signal_value|input_event_ids|max_input_received_time|max_input_sequence"
    );
    print!("{}", output.predictions.transcript());
    println!(
        "transcript_hash={:016x}",
        output.predictions.transcript_hash()
    );
    println!("labels_seen={}", output.labels_seen);
    Ok(())
}

fn check(args: &[String]) -> Result<(), Box<dyn Error>> {
    let (path, options) = parse_check_args(args)?;
    let events = load_events(path)?;
    let report = run_adversarial_checks_with_options(&events, options);

    print!("{}", format_check_report(&report));

    if report.passed() {
        Ok(())
    } else {
        Err("one or more adversarial checks failed".into())
    }
}

fn compare_leaky(path: &str) -> Result<(), Box<dyn Error>> {
    let events = load_events(path)?;
    let received_time = ReplayEngine::new().replay_with_order(
        &events,
        ReplayOptions::default(),
        ReplayOrder::ReceivedTime,
    )?;
    let observed_time = ReplayEngine::new().replay_with_order(
        &events,
        ReplayOptions::default(),
        ReplayOrder::ObservedTimeLeaky,
    )?;
    let event_labels: BTreeMap<_, _> = events
        .iter()
        .map(|event| (event.event_key, event.event_id.clone()))
        .collect();

    println!("compare-leaky path={path} events={}", events.len());
    print_engine_comparison("received-time replay", &received_time, &event_labels);
    println!();
    print_engine_comparison(
        "observed-time replay (leaky baseline)",
        &observed_time,
        &event_labels,
    );

    let correct_impossible = impossible_predictions(&received_time);
    let leaky_impossible = impossible_predictions(&observed_time);

    if !correct_impossible.is_empty() {
        return Err("received-time replay produced impossible predictions".into());
    }

    if leaky_impossible.is_empty() {
        return Err("observed-time leaky baseline produced no impossible predictions".into());
    }

    Ok(())
}

fn bench(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut events = 1_000_000_usize;
    let mut symbols = 1_024_usize;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--events" => {
                events = parse_flag_value(args, index, "--events")?;
                index += 2;
            }
            "--symbols" => {
                symbols = parse_flag_value(args, index, "--symbols")?;
                index += 2;
            }
            other => return Err(format!("unknown bench argument: {other}").into()),
        }
    }

    println!("bench events={events} symbols={symbols}");
    for result in run_representation_benchmark(events, symbols) {
        println!(
            "representation=\"{}\" events={} symbols={} elapsed_ms={:.3} events_per_second={:.0} checksum={}",
            result.representation,
            result.events,
            result.symbols,
            result.elapsed_ns as f64 / 1_000_000.0,
            result.events_per_second,
            result.checksum
        );
    }

    Ok(())
}

fn parse_check_args(args: &[String]) -> Result<(&str, CheckOptions), Box<dyn Error>> {
    let mut path = "examples/late-arrival.pipe";
    let mut options = CheckOptions::sampled(32);
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--max-cutoffs" => {
                let max_cutoffs = parse_arg(args, index, "--max-cutoffs")?;
                if max_cutoffs == 0 {
                    return Err("--max-cutoffs must be greater than 0".into());
                }
                options = CheckOptions::sampled(max_cutoffs);
                index += 2;
            }
            "--exhaustive" => {
                options = CheckOptions::exhaustive();
                index += 1;
            }
            value if value.starts_with("--") => {
                return Err(format!("unknown check argument: {value}").into());
            }
            value => {
                path = value;
                index += 1;
            }
        }
    }

    Ok((path, options))
}

fn generate(args: &[String]) -> Result<(), Box<dyn Error>> {
    let (config, out) = parse_generate_args(args, false)?;
    let stream = generate_events(&config);
    let contents = stream.to_pipe_string();

    if let Some(path) = out {
        write_file(&path, &contents)?;
        println!(
            "generated path={} scenario={} seed={} data_events={} rows={} symbols={} late_updates={} corrections={} predictions={}",
            path.display(),
            stream.stats.scenario.as_str(),
            stream.stats.seed,
            stream.stats.data_events,
            stream.stats.rows,
            stream.stats.symbols,
            stream.stats.late_updates,
            stream.stats.corrections,
            stream.stats.predictions
        );
    } else {
        print!("{contents}");
    }

    Ok(())
}

fn run_suite(args: &[String]) -> Result<(), Box<dyn Error>> {
    let (config, out) = parse_generate_args(args, true)?;
    let out_dir = out.unwrap_or_else(|| {
        PathBuf::from(format!(
            "runs/{}-seed-{}",
            config.scenario.as_str(),
            config.seed
        ))
    });

    fs::create_dir_all(&out_dir)?;

    let stream = generate_events(&config);
    let events_path = out_dir.join("events.pipe");
    let predictions_path = out_dir.join("predictions.pipe");
    let checks_path = out_dir.join("checks.txt");
    let summary_path = out_dir.join("summary.md");

    write_file(&events_path, &stream.to_pipe_string())?;

    let replay = ReplayEngine::new().replay(&stream.events, ReplayOptions::default())?;
    let report = run_adversarial_checks_with_options(&stream.events, CheckOptions::sampled(32));

    write_file(&predictions_path, &format_prediction_output(&replay))?;
    write_file(&checks_path, &format_check_report(&report))?;
    write_file(
        &summary_path,
        &format_suite_summary(&stream, &replay, &report),
    )?;

    println!(
        "suite out={} scenario={} seed={} rows={} predictions={} transcript_hash={:016x}",
        out_dir.display(),
        stream.stats.scenario.as_str(),
        stream.stats.seed,
        stream.stats.rows,
        replay.predictions.records().len(),
        replay.predictions.transcript_hash()
    );
    println!("events={}", events_path.display());
    println!("predictions={}", predictions_path.display());
    println!("checks={}", checks_path.display());
    println!("summary={}", summary_path.display());

    if report.passed() {
        Ok(())
    } else {
        Err("one or more adversarial checks failed".into())
    }
}

fn parse_flag_value(args: &[String], index: usize, flag: &str) -> Result<usize, Box<dyn Error>> {
    let value = args
        .get(index + 1)
        .ok_or_else(|| format!("{flag} requires a value"))?;
    Ok(value.parse()?)
}

fn parse_generate_args(
    args: &[String],
    out_is_directory: bool,
) -> Result<(GenerateConfig, Option<PathBuf>), Box<dyn Error>> {
    let scenario = find_string_flag(args, "--scenario")
        .map(|value| {
            Scenario::parse(value).ok_or_else(|| {
                format!("unknown scenario {value}; expected clean, late-heavy, or correction-heavy")
            })
        })
        .transpose()?
        .unwrap_or(Scenario::LateHeavy);

    let mut config = GenerateConfig::for_scenario(scenario);
    let mut out = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--scenario" => {
                required_arg(args, index, "--scenario")?;
                index += 2;
            }
            "--events" => {
                config.events = parse_arg(args, index, "--events")?;
                index += 2;
            }
            "--symbols" => {
                config.symbols = parse_arg(args, index, "--symbols")?;
                index += 2;
            }
            "--late-rate" => {
                config.late_rate = parse_arg(args, index, "--late-rate")?;
                index += 2;
            }
            "--correction-rate" => {
                config.correction_rate = parse_arg(args, index, "--correction-rate")?;
                index += 2;
            }
            "--label-rate" => {
                config.label_rate = parse_arg(args, index, "--label-rate")?;
                index += 2;
            }
            "--prediction-interval" => {
                config.prediction_interval = parse_arg(args, index, "--prediction-interval")?;
                index += 2;
            }
            "--max-lag" => {
                config.max_lag = parse_arg(args, index, "--max-lag")?;
                index += 2;
            }
            "--seed" => {
                config.seed = parse_arg(args, index, "--seed")?;
                index += 2;
            }
            "--out" => {
                out = Some(PathBuf::from(required_arg(args, index, "--out")?));
                index += 2;
            }
            "--ordered" => {
                config.shuffle_physical_order = false;
                index += 1;
            }
            "--shuffle" => {
                config.shuffle_physical_order = true;
                index += 1;
            }
            other => {
                let target = if out_is_directory {
                    "run-suite"
                } else {
                    "generate"
                };
                return Err(format!("unknown {target} argument: {other}").into());
            }
        }
    }

    Ok((config, out))
}

fn find_string_flag<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|window| window[0] == flag)
        .map(|window| window[1].as_str())
}

fn required_arg<'a>(
    args: &'a [String],
    index: usize,
    flag: &str,
) -> Result<&'a str, Box<dyn Error>> {
    args.get(index + 1)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} requires a value").into())
}

fn parse_arg<T>(args: &[String], index: usize, flag: &str) -> Result<T, Box<dyn Error>>
where
    T: std::str::FromStr,
    T::Err: Error + 'static,
{
    Ok(required_arg(args, index, flag)?.parse()?)
}

fn load_events(path: &str) -> Result<Vec<asof_replay_core::Event>, Box<dyn Error>> {
    let input = fs::read_to_string(path)?;
    Ok(parse_pipe_events(&input)?)
}

fn write_file(path: &Path, contents: &str) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

fn format_prediction_output(output: &ReplayOutput) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "prediction_time|prediction_sequence|symbol|signal_value|input_event_ids|max_input_received_time|max_input_sequence"
    );
    text.push_str(&output.predictions.transcript());
    let _ = writeln!(
        text,
        "transcript_hash={:016x}",
        output.predictions.transcript_hash()
    );
    let _ = writeln!(text, "labels_seen={}", output.labels_seen);
    text
}

fn print_engine_comparison(
    name: &str,
    output: &ReplayOutput,
    event_labels: &BTreeMap<asof_replay_core::EventKey, String>,
) {
    let impossible = impossible_predictions(output);
    let status = if impossible.is_empty() {
        "PASS"
    } else {
        "FAIL"
    };

    println!("{name}: {status}");
    println!(
        "  transcript_hash={:016x}",
        output.predictions.transcript_hash()
    );
    println!("  impossible_predictions={}", impossible.len());

    for record in impossible.iter().take(8) {
        println!("  {}", record.canonical_line(event_labels));
        println!(
            "  impossible: input received at ({}, {}) was used by prediction at ({}, {})",
            record.max_input_received_time,
            record.max_input_sequence,
            record.prediction_time,
            record.prediction_sequence
        );
    }
}

fn impossible_predictions(output: &ReplayOutput) -> Vec<&PredictionRecord> {
    output
        .predictions
        .records()
        .iter()
        .filter(|record| record.uses_future_input())
        .collect()
}

fn format_check_report(report: &CheckReport) -> String {
    let mut text = String::new();
    for result in &report.results {
        let status = if result.passed { "PASS" } else { "FAIL" };
        let _ = writeln!(text, "{status} {} - {}", result.name, result.detail);
    }
    text
}

fn format_suite_summary(
    stream: &GeneratedStream,
    replay: &ReplayOutput,
    report: &CheckReport,
) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "# asof-replay run suite");
    let _ = writeln!(text);
    let _ = writeln!(text, "- scenario: {}", stream.stats.scenario.as_str());
    let _ = writeln!(text, "- seed: {}", stream.stats.seed);
    let _ = writeln!(text, "- data_events: {}", stream.stats.data_events);
    let _ = writeln!(text, "- rows: {}", stream.stats.rows);
    let _ = writeln!(text, "- symbols: {}", stream.stats.symbols);
    let _ = writeln!(text, "- late_updates: {}", stream.stats.late_updates);
    let _ = writeln!(text, "- corrections: {}", stream.stats.corrections);
    let _ = writeln!(text, "- labels: {}", stream.stats.labels);
    let _ = writeln!(
        text,
        "- transcript_hash: {:016x}",
        replay.predictions.transcript_hash()
    );
    let _ = writeln!(text);
    let _ = writeln!(text, "## Checks");
    let _ = writeln!(text);
    for result in &report.results {
        let status = if result.passed { "PASS" } else { "FAIL" };
        let _ = writeln!(text, "- {status} {}: {}", result.name, result.detail);
    }
    text
}

fn print_help() {
    println!("asof-replay");
    println!();
    println!("usage:");
    println!("  asof-replay replay [path]");
    println!("  asof-replay check [path] [--max-cutoffs N|--exhaustive]");
    println!("  asof-replay compare-leaky [path]");
    println!("  asof-replay generate [--scenario late-heavy] [--events N] [--symbols N] [--late-rate R] [--correction-rate R] [--seed N] [--out path]");
    println!("  asof-replay run-suite [--scenario late-heavy] [--events N] [--symbols N] [--late-rate R] [--correction-rate R] [--seed N] [--out dir]");
    println!("  asof-replay bench [--events N] [--symbols N]");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_cutoffs_zero_is_rejected() {
        let args = vec!["--max-cutoffs".to_string(), "0".to_string()];
        let error = parse_check_args(&args).unwrap_err().to_string();

        assert!(error.contains("--max-cutoffs must be greater than 0"));
    }
}
