use asof_causality_core::{
    generate_events, parse_pipe_events, run_adversarial_checks_with_options_for_signal,
    run_representation_benchmark, CheckOptions, CheckReport, GenerateConfig, GeneratedStream,
    LastFeatureSentimentSignal, ReplayEngine, ReplayOptions, ReplayOrder, ReplayOutput, Scenario,
    WindowedFeatureSentimentSignal,
};
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("replay") => replay(&args[2..]),
        Some("check") => check(&args[2..]),
        Some(command) if is_negative_control_command(command) => negative_control(&args[2..]),
        Some("generate") => generate(&args[2..]),
        Some("run-suite") => run_suite(&args[2..]),
        Some("bench") => bench(&args[2..]),
        _ => {
            print_help();
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SignalChoice {
    #[default]
    LastFeatureSentiment,
    WindowedFeatureSentiment,
}

impl SignalChoice {
    fn parse(value: &str) -> Result<Self, Box<dyn Error>> {
        match value {
            "last-feature-sentiment" => Ok(Self::LastFeatureSentiment),
            "windowed-feature-sentiment" => Ok(Self::WindowedFeatureSentiment),
            other => Err(format!(
                "unknown signal {other}; expected last-feature-sentiment or windowed-feature-sentiment"
            )
            .into()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::LastFeatureSentiment => "last-feature-sentiment",
            Self::WindowedFeatureSentiment => "windowed-feature-sentiment",
        }
    }
}

fn is_negative_control_command(command: &str) -> bool {
    matches!(command, "negative-control" | "compare-leaky")
}

fn replay(args: &[String]) -> Result<(), Box<dyn Error>> {
    let (path, signal) = parse_path_signal_args(args, "examples/late-arrival.pipe", "replay")?;
    let events = load_events(&path)?;
    let output = replay_with_signal(
        signal,
        &events,
        ReplayOptions::default(),
        ReplayOrder::ReceivedTime,
    )?;

    println!(
        "replay path={path} signal={} events={}",
        signal.as_str(),
        output.replayed_events
    );
    println!("prediction_replay_key|symbol|signal_value|input_event_ids|max_input_replay_key");
    print!("{}", output.predictions.transcript());
    println!(
        "transcript_hash={:016x}",
        output.predictions.transcript_hash()
    );
    println!("outcomes_seen={}", output.outcomes_seen);
    Ok(())
}

fn check(args: &[String]) -> Result<(), Box<dyn Error>> {
    let (path, options, signal) = parse_check_args(args)?;
    let events = load_events(path)?;
    let report = run_checks_with_signal(signal, &events, options);

    print!("{}", format_check_report(&report));

    if report.passed() {
        Ok(())
    } else {
        Err("one or more adversarial checks failed".into())
    }
}

fn negative_control(args: &[String]) -> Result<(), Box<dyn Error>> {
    let (path, signal) = parse_path_signal_args(
        args,
        "examples/lookahead-negative-control.pipe",
        "negative-control",
    )?;
    let events = load_events(&path)?;
    let received_time = replay_with_signal(
        signal,
        &events,
        ReplayOptions::default(),
        ReplayOrder::ReceivedTime,
    )?;
    let observed_time = replay_with_signal(
        signal,
        &events,
        ReplayOptions::default(),
        ReplayOrder::ObservedTimeLeaky,
    )?;
    let event_labels: BTreeMap<_, _> = events
        .iter()
        .map(|event| (event.event_key, event.event_id.clone()))
        .collect();

    println!(
        "negative-control path={path} signal={} events={}",
        signal.as_str(),
        events.len()
    );
    print_engine_comparison("received-time replay", &received_time, &event_labels);
    println!();
    print_engine_comparison(
        "observed-time replay (leaky baseline)",
        &observed_time,
        &event_labels,
    );

    let correct_impossible = received_time.predictions.impossible_predictions();
    let leaky_impossible = observed_time.predictions.impossible_predictions();

    if !correct_impossible.is_empty() {
        return Err("received-time replay produced impossible predictions".into());
    }

    if leaky_impossible.is_empty() {
        println!();
        println!("note: the leaky baseline did not violate the invariant on this fixture");
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

fn replay_with_signal(
    signal: SignalChoice,
    events: &[asof_causality_core::Event],
    options: ReplayOptions,
    order: ReplayOrder,
) -> Result<ReplayOutput, asof_causality_core::ReplayError> {
    match signal {
        SignalChoice::LastFeatureSentiment => ReplayEngine::with_signal(LastFeatureSentimentSignal)
            .replay_with_order(events, options, order),
        SignalChoice::WindowedFeatureSentiment => {
            ReplayEngine::with_signal(WindowedFeatureSentimentSignal::default())
                .replay_with_order(events, options, order)
        }
    }
}

fn run_checks_with_signal(
    signal: SignalChoice,
    events: &[asof_causality_core::Event],
    options: CheckOptions,
) -> CheckReport {
    match signal {
        SignalChoice::LastFeatureSentiment => run_adversarial_checks_with_options_for_signal(
            events,
            options,
            LastFeatureSentimentSignal,
        ),
        SignalChoice::WindowedFeatureSentiment => run_adversarial_checks_with_options_for_signal(
            events,
            options,
            WindowedFeatureSentimentSignal::default(),
        ),
    }
}

fn parse_path_signal_args(
    args: &[String],
    default_path: &str,
    command: &str,
) -> Result<(String, SignalChoice), Box<dyn Error>> {
    let mut path = default_path.to_string();
    let mut signal = SignalChoice::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--signal" => {
                signal = SignalChoice::parse(required_arg(args, index, "--signal")?)?;
                index += 2;
            }
            value if value.starts_with("--") => {
                return Err(format!("unknown {command} argument: {value}").into());
            }
            value => {
                path = value.to_string();
                index += 1;
            }
        }
    }

    Ok((path, signal))
}

fn parse_check_args(args: &[String]) -> Result<(&str, CheckOptions, SignalChoice), Box<dyn Error>> {
    let mut path = "examples/late-arrival.pipe";
    let mut options = CheckOptions::sampled(32);
    let mut signal = SignalChoice::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--signal" => {
                signal = SignalChoice::parse(required_arg(args, index, "--signal")?)?;
                index += 2;
            }
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

    Ok((path, options, signal))
}

fn generate(args: &[String]) -> Result<(), Box<dyn Error>> {
    let (config, out, _) = parse_generate_args(args, false)?;
    let stream = generate_events(&config);
    let contents = stream.to_pipe_string();

    if let Some(path) = out {
        write_file(&path, &contents)?;
        println!(
            "generated path={} scenario={} seed={} data_events={} rows={} symbols={} late_updates={} feature_corrections={} predictions={}",
            path.display(),
            stream.stats.scenario.as_str(),
            stream.stats.seed,
            stream.stats.data_events,
            stream.stats.rows,
            stream.stats.symbols,
            stream.stats.late_updates,
            stream.stats.feature_corrections,
            stream.stats.predictions
        );
    } else {
        print!("{contents}");
    }

    Ok(())
}

fn run_suite(args: &[String]) -> Result<(), Box<dyn Error>> {
    let (config, out, signal) = parse_generate_args(args, true)?;
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
    let manifest_path = out_dir.join("manifest.json");

    let events_output = stream.to_pipe_string();
    write_file(&events_path, &events_output)?;

    let replay = replay_with_signal(
        signal,
        &stream.events,
        ReplayOptions::default(),
        ReplayOrder::ReceivedTime,
    )?;
    let report = run_checks_with_signal(signal, &stream.events, CheckOptions::sampled(32));

    let predictions_output = format_prediction_output(&replay);
    let checks_output = format_check_report(&report);
    let summary_output = format_suite_summary(&stream, signal, &replay, &report);
    let manifest_output = format_run_manifest(RunManifestInputs {
        config: &config,
        stream: &stream,
        signal,
        replay: &replay,
        report: &report,
        events_output: &events_output,
        predictions_output: &predictions_output,
        checks_output: &checks_output,
    });

    write_file(&predictions_path, &predictions_output)?;
    write_file(&checks_path, &checks_output)?;
    write_file(&summary_path, &summary_output)?;
    write_file(&manifest_path, &manifest_output)?;

    println!(
        "suite out={} scenario={} signal={} seed={} rows={} predictions={} transcript_hash={:016x}",
        out_dir.display(),
        stream.stats.scenario.as_str(),
        signal.as_str(),
        stream.stats.seed,
        stream.stats.rows,
        replay.predictions.records().len(),
        replay.predictions.transcript_hash()
    );
    println!("events={}", events_path.display());
    println!("predictions={}", predictions_path.display());
    println!("checks={}", checks_path.display());
    println!("summary={}", summary_path.display());
    println!("manifest={}", manifest_path.display());

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
) -> Result<(GenerateConfig, Option<PathBuf>, SignalChoice), Box<dyn Error>> {
    let scenario = find_string_flag(args, "--scenario")
        .map(|value| {
            Scenario::parse(value).ok_or_else(|| {
                format!(
                    "unknown scenario {value}; expected clean, late-heavy, or feature-correction-heavy"
                )
            })
        })
        .transpose()?
        .unwrap_or(Scenario::LateHeavy);

    let mut config = GenerateConfig::for_scenario(scenario);
    let mut out = None;
    let mut signal = SignalChoice::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--signal" if out_is_directory => {
                signal = SignalChoice::parse(required_arg(args, index, "--signal")?)?;
                index += 2;
            }
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
            flag @ ("--feature-correction-rate" | "--correction-rate") => {
                config.feature_correction_rate = parse_arg(args, index, flag)?;
                index += 2;
            }
            flag @ ("--outcome-rate" | "--label-rate") => {
                config.outcome_rate = parse_arg(args, index, flag)?;
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

    Ok((config, out, signal))
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

fn load_events(path: &str) -> Result<Vec<asof_causality_core::Event>, Box<dyn Error>> {
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
        "prediction_replay_key|symbol|signal_value|input_event_ids|max_input_replay_key"
    );
    text.push_str(&output.predictions.transcript());
    let _ = writeln!(
        text,
        "transcript_hash={:016x}",
        output.predictions.transcript_hash()
    );
    let _ = writeln!(text, "outcomes_seen={}", output.outcomes_seen);
    text
}

fn print_engine_comparison(
    name: &str,
    output: &ReplayOutput,
    event_labels: &BTreeMap<asof_causality_core::EventKey, String>,
) {
    let impossible = output.predictions.impossible_predictions();
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
            "  impossible: input replay key {} was used by prediction key {}",
            record.max_input_replay_key(event_labels),
            record
                .canonical_line(event_labels)
                .split('|')
                .next()
                .unwrap_or("-")
        );
    }
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
    signal: SignalChoice,
    replay: &ReplayOutput,
    report: &CheckReport,
) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "# asof-causality run suite");
    let _ = writeln!(text);
    let _ = writeln!(text, "- scenario: {}", stream.stats.scenario.as_str());
    let _ = writeln!(text, "- signal: {}", signal.as_str());
    let _ = writeln!(text, "- seed: {}", stream.stats.seed);
    let _ = writeln!(text, "- data_events: {}", stream.stats.data_events);
    let _ = writeln!(text, "- rows: {}", stream.stats.rows);
    let _ = writeln!(text, "- symbols: {}", stream.stats.symbols);
    let _ = writeln!(text, "- late_updates: {}", stream.stats.late_updates);
    let _ = writeln!(
        text,
        "- feature_corrections: {}",
        stream.stats.feature_corrections
    );
    let _ = writeln!(text, "- outcomes: {}", stream.stats.outcomes);
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

struct RunManifestInputs<'a> {
    config: &'a GenerateConfig,
    stream: &'a GeneratedStream,
    signal: SignalChoice,
    replay: &'a ReplayOutput,
    report: &'a CheckReport,
    events_output: &'a str,
    predictions_output: &'a str,
    checks_output: &'a str,
}

fn format_run_manifest(inputs: RunManifestInputs<'_>) -> String {
    let mut text = String::new();
    let git_commit = current_git_commit();
    let rust_toolchain = current_rustc_version();
    let checks_passed = inputs.report.passed();
    let signal_version = format!(
        "{}:{}",
        git_commit.as_deref().unwrap_or("unknown-commit"),
        inputs.signal.as_str()
    );

    let _ = writeln!(text, "{{");
    let _ = writeln!(text, "  \"schema_version\": 1,");
    let _ = writeln!(text, "  \"tool\": \"asof-causality\",");
    let _ = writeln!(
        text,
        "  \"code_commit_hash\": {},",
        json_optional(&git_commit)
    );
    let _ = writeln!(
        text,
        "  \"rust_toolchain\": {},",
        json_optional(&rust_toolchain)
    );
    let _ = writeln!(
        text,
        "  \"scenario\": \"{}\",",
        inputs.stream.stats.scenario.as_str()
    );
    let _ = writeln!(text, "  \"signal\": \"{}\",", inputs.signal.as_str());
    let _ = writeln!(text, "  \"seed\": {},", inputs.stream.stats.seed);
    let _ = writeln!(text, "  \"events_requested\": {},", inputs.config.events);
    let _ = writeln!(text, "  \"rows\": {},", inputs.stream.stats.rows);
    let _ = writeln!(text, "  \"symbols\": {},", inputs.stream.stats.symbols);
    let _ = writeln!(text, "  \"late_rate\": {},", inputs.config.late_rate);
    let _ = writeln!(
        text,
        "  \"feature_correction_rate\": {},",
        inputs.config.feature_correction_rate
    );
    let _ = writeln!(text, "  \"outcome_rate\": {},", inputs.config.outcome_rate);
    let _ = writeln!(
        text,
        "  \"data_fixture_hash\": \"{:016x}\",",
        asof_causality_core::fnv1a64(inputs.events_output.as_bytes())
    );
    let _ = writeln!(
        text,
        "  \"prediction_output_hash\": \"{:016x}\",",
        asof_causality_core::fnv1a64(inputs.predictions_output.as_bytes())
    );
    let _ = writeln!(
        text,
        "  \"checks_output_hash\": \"{:016x}\",",
        asof_causality_core::fnv1a64(inputs.checks_output.as_bytes())
    );
    let _ = writeln!(
        text,
        "  \"signal_version_hash\": \"{:016x}\",",
        asof_causality_core::fnv1a64(signal_version.as_bytes())
    );
    let _ = writeln!(
        text,
        "  \"transcript_hash\": \"{:016x}\",",
        inputs.replay.predictions.transcript_hash()
    );
    let _ = writeln!(text, "  \"checks_passed\": {checks_passed}");
    let _ = writeln!(text, "}}");
    text
}

fn current_git_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let commit = String::from_utf8(output.stdout).ok()?;
    Some(commit.trim().to_string()).filter(|commit| !commit.is_empty())
}

fn current_rustc_version() -> Option<String> {
    let output = Command::new("rustc").arg("--version").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let version = String::from_utf8(output.stdout).ok()?;
    Some(version.trim().to_string()).filter(|version| !version.is_empty())
}

fn json_optional(value: &Option<String>) -> String {
    value
        .as_ref()
        .map(|value| format!("\"{}\"", json_escape(value)))
        .unwrap_or_else(|| "null".to_string())
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn print_help() {
    println!("asof-causality");
    println!();
    println!("usage:");
    println!("  asof-causality replay [path] [--signal name]");
    println!("  asof-causality check [path] [--signal name] [--max-cutoffs N|--exhaustive]");
    println!("  asof-causality negative-control [path] [--signal name]");
    println!("  asof-causality generate [--scenario late-heavy] [--events N] [--symbols N] [--late-rate R] [--feature-correction-rate R] [--outcome-rate R] [--seed N] [--out path]");
    println!("  asof-causality run-suite [--scenario late-heavy] [--signal name] [--events N] [--symbols N] [--late-rate R] [--feature-correction-rate R] [--outcome-rate R] [--seed N] [--out dir]");
    println!("  asof-causality bench [--events N] [--symbols N]");
    println!();
    println!("signals:");
    println!("  last-feature-sentiment (default)");
    println!("  windowed-feature-sentiment");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn negative_control_command_has_legacy_alias() {
        assert!(is_negative_control_command("negative-control"));
        assert!(is_negative_control_command("compare-leaky"));
        assert!(!is_negative_control_command("run-suite"));
    }

    #[test]
    fn parses_signal_for_replay_like_commands() {
        let (path, signal) = parse_path_signal_args(
            &args(&[
                "examples/lookahead-negative-control.pipe",
                "--signal",
                "windowed-feature-sentiment",
            ]),
            "default.pipe",
            "negative-control",
        )
        .unwrap();

        assert_eq!(path, "examples/lookahead-negative-control.pipe");
        assert_eq!(signal, SignalChoice::WindowedFeatureSentiment);
    }

    #[test]
    fn parses_signal_for_run_suite() {
        let (_, _, signal) =
            parse_generate_args(&args(&["--signal", "windowed-feature-sentiment"]), true).unwrap();

        assert_eq!(signal, SignalChoice::WindowedFeatureSentiment);
    }

    #[test]
    fn rejects_signal_for_generate() {
        let error = parse_generate_args(&args(&["--signal", "windowed-feature-sentiment"]), false)
            .unwrap_err()
            .to_string();

        assert!(error.contains("unknown generate argument"));
    }

    #[test]
    fn rejects_zero_max_cutoffs() {
        let error = parse_check_args(&args(&["--max-cutoffs", "0"]))
            .unwrap_err()
            .to_string();

        assert_eq!(error, "--max-cutoffs must be greater than 0");
    }
}
