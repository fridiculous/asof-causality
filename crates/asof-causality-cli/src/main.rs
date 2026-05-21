use asof_causality_core::{
    generate_events, parse_pipe_events, run_adversarial_checks_with_options_for_signal,
    run_representation_benchmark, run_sensitivity_sweep, CheckOptions, CheckReport, Event,
    EventKey, EventRole, GenerateConfig, GeneratedStream, LastFeatureSentimentSignal, PolicyKind,
    PolicyPoint, PolicyRun, ReplayEngine, ReplayOptions, ReplayOrder, ReplayOutput, Scenario,
    SensitivityPolicyResult, SensitivitySweep, SymbolId, WindowedFeatureSentimentSignal,
    WindowedZScoreSignal,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Number, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::fmt::Write;
use std::fs;
use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn main() {
    if let Err(error) = run() {
        let _ = io::stdout().flush();
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("replay") => replay(&args[2..]),
        Some("check") => check(&args[2..]),
        Some("audit") => audit(&args[2..]),
        Some(command) if is_negative_control_command(command) => negative_control(&args[2..]),
        Some("generate") => generate(&args[2..]),
        Some("run-suite") => run_suite(&args[2..]),
        Some("sensitivity") => sensitivity(&args[2..]),
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
    WindowedZScore,
}

impl SignalChoice {
    fn parse(value: &str) -> Result<Self, Box<dyn Error>> {
        match value {
            "last-feature-sentiment" => Ok(Self::LastFeatureSentiment),
            "windowed-feature-sentiment" => Ok(Self::WindowedFeatureSentiment),
            "windowed-zscore" => Ok(Self::WindowedZScore),
            other => Err(format!(
                "unknown signal {other}; expected last-feature-sentiment, windowed-feature-sentiment, or windowed-zscore"
            )
            .into()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::LastFeatureSentiment => "last-feature-sentiment",
            Self::WindowedFeatureSentiment => "windowed-feature-sentiment",
            Self::WindowedZScore => "windowed-zscore",
        }
    }

    fn config_descriptor(self) -> String {
        match self {
            Self::LastFeatureSentiment => String::new(),
            Self::WindowedFeatureSentiment => {
                format!("window={}", WindowedFeatureSentimentSignal::DEFAULT_WINDOW)
            }
            Self::WindowedZScore => format!(
                "window={};threshold={}",
                WindowedZScoreSignal::DEFAULT_WINDOW,
                WindowedZScoreSignal::DEFAULT_THRESHOLD
            ),
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
    let replay = replay_with_signal(
        signal,
        &events,
        ReplayOptions::default(),
        ReplayOrder::ReceivedTime,
    )
    .ok();

    print_check_stdout(path, signal, options, &events, &report, replay.as_ref());

    if report.passed() {
        Ok(())
    } else {
        Err("one or more adversarial checks failed".into())
    }
}

fn audit(args: &[String]) -> Result<(), Box<dyn Error>> {
    let audit_args = parse_audit_args(args)?;
    let events = load_events(&audit_args.events_path)?;
    let output = replay_with_signal(
        audit_args.signal,
        &events,
        ReplayOptions::default(),
        ReplayOrder::ReceivedTime,
    )?;
    let stored_predictions = audit_args
        .stored_predictions_path
        .as_ref()
        .map(|path| load_stored_predictions(path))
        .transpose()?;
    let outcomes = audit_args
        .outcomes_path
        .as_ref()
        .map(|path| load_outcome_attributions(path))
        .transpose()?
        .unwrap_or_default();
    let jsonl = format_audit_jsonl(
        audit_args.signal,
        &output,
        stored_predictions.as_ref(),
        &outcomes,
        audit_args.allow_missing_recipe_hash,
    );
    let summary = summarize_audit(
        &output,
        stored_predictions.as_ref(),
        &outcomes,
        audit_args.allow_missing_recipe_hash,
    );

    if let Some(path) = audit_args.out {
        write_file(&path, &jsonl)?;
        println!(
            "audit path={} signal={} records={} causally_valid={} matched_stored_predictions={} outcomes_attached={} out={}",
            audit_args.events_path,
            audit_args.signal.as_str(),
            output.predictions.records().len(),
            summary.causally_valid,
            summary.stored_match_summary(),
            summary.outcomes_attached,
            path.display()
        );
    } else {
        print!("{jsonl}");
    }

    if summary.passed() {
        Ok(())
    } else {
        Err(summary.failure_message().into())
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
    let labels = EventLabels::new(&events);

    print_negative_control_stdout(
        &path,
        signal,
        &events,
        &received_time,
        &observed_time,
        &labels,
    );

    let correct_impossible = received_time.predictions.impossible_predictions();

    if !correct_impossible.is_empty() {
        return Err("received-time replay produced impossible predictions".into());
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SensitivityArgs {
    events_path: String,
    signal: SignalChoice,
    out_dir: PathBuf,
    details: bool,
    scenario: SensitivityScenario,
    late_arrival_buckets: Option<LateArrivalBucketSpec>,
    late_arrival_steps: usize,
    policies: Vec<PolicyPoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SensitivityScenario {
    Custom,
    Lookahead,
    LateArrivals,
}

impl SensitivityScenario {
    fn parse(value: &str) -> Result<Self, Box<dyn Error>> {
        match value {
            "custom" => Ok(Self::Custom),
            "lookahead" => Ok(Self::Lookahead),
            "late-arrivals" | "late_arrivals" => Ok(Self::LateArrivals),
            other => Err(format!(
                "unknown sensitivity scenario {other}; expected custom, lookahead, or late-arrivals"
            )
            .into()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::Lookahead => "lookahead",
            Self::LateArrivals => "late-arrivals",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LateArrivalBucketSpec {
    Auto,
}

fn sensitivity(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut sensitivity_args = parse_sensitivity_args(args)?;
    let input = fs::read_to_string(&sensitivity_args.events_path)?;
    let events = parse_pipe_events(&input)?;
    append_late_arrival_policies(&mut sensitivity_args, &events)?;
    let sweep =
        run_sensitivity_with_signal(sensitivity_args.signal, &events, &sensitivity_args.policies)?;

    fs::create_dir_all(&sensitivity_args.out_dir)?;
    let summary_path = sensitivity_args.out_dir.join("summary.jsonl");
    let details_path = sensitivity_args.out_dir.join("details.jsonl");
    let manifest_path = sensitivity_args.out_dir.join("manifest.json");
    let sensitivity_curve_svg_path = sensitivity_args.out_dir.join("sensitivity-curve.svg");
    let flip_rate_svg_path = sensitivity_args.out_dir.join("flip-rate.svg");
    let input_change_svg_path = sensitivity_args.out_dir.join("input-change.svg");
    let late_arrival_impact_svg_path = sensitivity_args.out_dir.join("late-arrival-impact.svg");

    let summary_jsonl = format_sensitivity_summary_jsonl(sensitivity_args.signal, &sweep);
    write_file(&summary_path, &summary_jsonl)?;
    let sensitivity_curve_svg = if sweep_has_sensitivity_curve_points(&sweep) {
        let svg = format_sensitivity_curve_svg(&sweep);
        write_file(&sensitivity_curve_svg_path, &svg)?;
        Some(svg)
    } else {
        None
    };
    let flip_rate_svg = format_sensitivity_flip_rate_svg(&sweep);
    write_file(&flip_rate_svg_path, &flip_rate_svg)?;
    let input_change_svg = format_sensitivity_input_change_svg(&sweep);
    write_file(&input_change_svg_path, &input_change_svg)?;
    let late_arrival_impact_svg = if sweep_has_late_arrival_bucket_policies(&sweep) {
        let svg = format_late_arrival_impact_svg(&sweep);
        write_file(&late_arrival_impact_svg_path, &svg)?;
        Some(svg)
    } else {
        None
    };

    let details_jsonl = if sensitivity_args.details {
        let details_jsonl = format_sensitivity_details_jsonl(&sweep);
        write_file(&details_path, &details_jsonl)?;
        Some(details_jsonl)
    } else {
        None
    };

    let manifest_json = format_sensitivity_manifest_json(SensitivityManifestInputs {
        events_path: &sensitivity_args.events_path,
        fixture_input: &input,
        signal: sensitivity_args.signal,
        sweep: &sweep,
        summary_path: &summary_path,
        summary_jsonl: &summary_jsonl,
        sensitivity_curve_svg_path: sensitivity_curve_svg
            .as_ref()
            .map(|_| sensitivity_curve_svg_path.as_path()),
        sensitivity_curve_svg: sensitivity_curve_svg.as_deref(),
        flip_rate_svg_path: &flip_rate_svg_path,
        flip_rate_svg: &flip_rate_svg,
        input_change_svg_path: &input_change_svg_path,
        input_change_svg: &input_change_svg,
        late_arrival_impact_svg_path: late_arrival_impact_svg
            .as_ref()
            .map(|_| late_arrival_impact_svg_path.as_path()),
        late_arrival_impact_svg: late_arrival_impact_svg.as_deref(),
        details_path: sensitivity_args.details.then_some(details_path.as_path()),
        details_jsonl: details_jsonl.as_deref(),
    });
    write_file(&manifest_path, &manifest_json)?;

    print_sensitivity_stdout(
        &sensitivity_args,
        &sweep,
        SensitivityArtifactPaths {
            summary: &summary_path,
            sensitivity_curve_svg: sensitivity_curve_svg
                .as_ref()
                .map(|_| sensitivity_curve_svg_path.as_path()),
            flip_rate_svg: &flip_rate_svg_path,
            input_change_svg: &input_change_svg_path,
            late_arrival_impact_svg: late_arrival_impact_svg
                .as_ref()
                .map(|_| late_arrival_impact_svg_path.as_path()),
            details: sensitivity_args.details.then_some(details_path.as_path()),
            manifest: &manifest_path,
        },
    );

    Ok(())
}

fn run_sensitivity_with_signal(
    signal: SignalChoice,
    events: &[Event],
    policies: &[PolicyPoint],
) -> Result<SensitivitySweep, asof_causality_core::SensitivityError> {
    match signal {
        SignalChoice::LastFeatureSentiment => {
            run_sensitivity_sweep(events, policies, LastFeatureSentimentSignal)
        }
        SignalChoice::WindowedFeatureSentiment => {
            run_sensitivity_sweep(events, policies, WindowedFeatureSentimentSignal::default())
        }
        SignalChoice::WindowedZScore => {
            run_sensitivity_sweep(events, policies, WindowedZScoreSignal::default())
        }
    }
}

fn parse_sensitivity_args(args: &[String]) -> Result<SensitivityArgs, Box<dyn Error>> {
    let mut events_path = "examples/late-arrival.pipe".to_string();
    let mut signal = SignalChoice::default();
    let mut out_dir = None;
    let mut details = false;
    let mut scenario = None;
    let mut late_arrival_buckets = None;
    let mut policies = Vec::new();
    let mut lookahead_range = None;
    let mut lookahead_steps = 20_usize;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--scenario" => {
                scenario = Some(SensitivityScenario::parse(required_arg(
                    args,
                    index,
                    "--scenario",
                )?)?);
                index += 2;
            }
            "--signal" => {
                signal = SignalChoice::parse(required_arg(args, index, "--signal")?)?;
                index += 2;
            }
            "--out" => {
                out_dir = Some(PathBuf::from(required_arg(args, index, "--out")?));
                index += 2;
            }
            "--details" => {
                details = true;
                index += 1;
            }
            "--observed-time-leaky" => {
                policies.push(PolicyPoint::observed_time_leaky());
                index += 1;
            }
            "--lookahead-range" => {
                if lookahead_range.is_some() {
                    return Err("sensitivity accepts only one lookahead range".into());
                }
                lookahead_range = Some(parse_lookahead_range(required_arg(
                    args,
                    index,
                    "--lookahead-range",
                )?)?);
                index += 2;
            }
            "--late-arrival-buckets" | "--buckets" => {
                let value = required_arg(args, index, args[index].as_str())?;
                if value != "auto" {
                    return Err(
                        "sensitivity late-arrival buckets currently supports only auto".into(),
                    );
                }
                late_arrival_buckets = Some(LateArrivalBucketSpec::Auto);
                index += 2;
            }
            "--steps" => {
                lookahead_steps = parse_arg(args, index, "--steps")?;
                if lookahead_steps == 0 {
                    return Err("sensitivity --steps must be greater than zero".into());
                }
                index += 2;
            }
            "--shift-features" => {
                let value = required_arg(args, index, "--shift-features")?;
                let shift = parse_integer_shift(value)?;
                policies.push(PolicyPoint::shift_features(
                    shift_features_policy_name(shift),
                    shift,
                ));
                index += 2;
            }
            value if value.starts_with("--") => {
                return Err(format!("unknown sensitivity argument: {value}").into());
            }
            value => {
                events_path = value.to_string();
                index += 1;
            }
        }
    }

    let scenario = scenario.unwrap_or_else(|| {
        if late_arrival_buckets.is_some() {
            SensitivityScenario::LateArrivals
        } else if lookahead_range.is_some() {
            SensitivityScenario::Lookahead
        } else {
            SensitivityScenario::Custom
        }
    });
    if scenario == SensitivityScenario::Lookahead && late_arrival_buckets.is_some() {
        return Err(
            "sensitivity --scenario lookahead cannot be combined with late-arrival buckets".into(),
        );
    }
    if scenario == SensitivityScenario::LateArrivals && lookahead_range.is_some() {
        return Err(
            "sensitivity --scenario late-arrivals cannot be combined with --lookahead-range".into(),
        );
    }
    if scenario == SensitivityScenario::LateArrivals && late_arrival_buckets.is_none() {
        late_arrival_buckets = Some(LateArrivalBucketSpec::Auto);
    }
    if scenario == SensitivityScenario::Lookahead && lookahead_range.is_none() {
        lookahead_range = Some(PercentRangeSpec {
            start_bps: 0,
            end_bps: 10_000,
        });
    }

    if let Some(range) = lookahead_range {
        let generated_policies = lookahead_range_policies(range, lookahead_steps)?;
        let insert_at = policies
            .iter()
            .position(|policy| policy.name == "observed_time_leaky")
            .unwrap_or(policies.len());
        policies.splice(insert_at..insert_at, generated_policies);
    } else if args.iter().any(|arg| arg == "--steps")
        && scenario != SensitivityScenario::LateArrivals
    {
        return Err(
            "sensitivity --steps requires --lookahead-range or --scenario lookahead".into(),
        );
    }

    let Some(out_dir) = out_dir else {
        return Err("sensitivity requires --out DIR".into());
    };
    if policies.is_empty() && late_arrival_buckets.is_none() {
        return Err("sensitivity requires at least one comparison policy".into());
    }

    Ok(SensitivityArgs {
        events_path,
        signal,
        out_dir,
        details,
        scenario,
        late_arrival_buckets,
        late_arrival_steps: lookahead_steps,
        policies,
    })
}

fn parse_integer_shift(value: &str) -> Result<i64, Box<dyn Error>> {
    if value
        .chars()
        .any(|character| character.is_ascii_alphabetic())
    {
        return Err(
            "typed duration shifts like -1d are deferred in sensitivity v1; use an integer offset"
                .into(),
        );
    }
    Ok(value.parse()?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PercentRangeSpec {
    start_bps: u16,
    end_bps: u16,
}

fn parse_lookahead_range(value: &str) -> Result<PercentRangeSpec, Box<dyn Error>> {
    let (start, end) = value
        .split_once("..=")
        .or_else(|| value.split_once(".."))
        .or_else(|| value.split_once(':'))
        .ok_or("expected lookahead range like 0..100")?;
    let start_bps = parse_percent_bps(start)?;
    let end_bps = parse_percent_bps(end)?;
    if start_bps > end_bps {
        return Err("sensitivity --lookahead-range start must be <= end".into());
    }
    Ok(PercentRangeSpec { start_bps, end_bps })
}

fn parse_percent_bps(value: &str) -> Result<u16, Box<dyn Error>> {
    let value = value.trim().trim_end_matches('%');
    if value.is_empty() {
        return Err("empty lookahead percentage".into());
    }
    let (whole, fractional) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty() || !whole.chars().all(|character| character.is_ascii_digit()) {
        return Err(format!("invalid lookahead percentage: {value}").into());
    }
    if !fractional
        .chars()
        .all(|character| character.is_ascii_digit())
    {
        return Err(format!("invalid lookahead percentage: {value}").into());
    }

    let whole: u16 = whole.parse()?;
    if whole > 100 {
        return Err("lookahead percentage must be between 0 and 100".into());
    }

    let mut fractional_digits = fractional.chars().take(2).collect::<String>();
    while fractional_digits.len() < 2 {
        fractional_digits.push('0');
    }
    let fractional_bps = if fractional_digits.is_empty() {
        0
    } else {
        fractional_digits.parse::<u16>()?
    };
    let bps = whole
        .checked_mul(100)
        .and_then(|value| value.checked_add(fractional_bps))
        .ok_or("lookahead percentage is out of range")?;
    if bps > 10_000 {
        return Err("lookahead percentage must be between 0 and 100".into());
    }
    Ok(bps)
}

fn lookahead_range_policies(
    spec: PercentRangeSpec,
    steps: usize,
) -> Result<Vec<PolicyPoint>, Box<dyn Error>> {
    let mut percentages = Vec::new();
    let delta = u128::from(spec.end_bps - spec.start_bps);
    for step in 0..=steps {
        let pct_bps = u128::from(spec.start_bps)
            + ((delta * step as u128) + (steps as u128 / 2)) / steps as u128;
        let pct_bps = pct_bps as u16;
        if pct_bps == 0 {
            continue;
        }
        if percentages.last().copied() != Some(pct_bps) {
            percentages.push(pct_bps);
        }
    }
    if percentages.is_empty() {
        return Err("sensitivity --lookahead-range produced no comparison policies".into());
    }

    Ok(percentages
        .into_iter()
        .map(|pct_bps| {
            PolicyPoint::leak_feature_lag_fraction(lookahead_policy_name(pct_bps), pct_bps)
        })
        .collect())
}

fn lookahead_policy_name(pct_bps: u16) -> String {
    format!("lookahead_{}pct", format_percent_bps_for_name(pct_bps))
}

fn append_late_arrival_policies(
    args: &mut SensitivityArgs,
    events: &[Event],
) -> Result<(), Box<dyn Error>> {
    if args.late_arrival_buckets.is_none() {
        return Ok(());
    }

    let generated = cumulative_late_arrival_policies(events, args.late_arrival_steps)?;
    let insert_at = args
        .policies
        .iter()
        .position(|policy| policy.name == "observed_time_leaky")
        .unwrap_or(args.policies.len());
    args.policies.splice(insert_at..insert_at, generated);
    Ok(())
}

fn cumulative_late_arrival_policies(
    events: &[Event],
    steps: usize,
) -> Result<Vec<PolicyPoint>, Box<dyn Error>> {
    let mut lags = events
        .iter()
        .filter(|event| {
            matches!(
                event.role,
                EventRole::Feature | EventRole::FeatureCorrection
            ) && event.received_time > event.observed_time
        })
        .map(|event| event.received_time - event.observed_time)
        .collect::<Vec<_>>();
    if lags.is_empty() {
        return Err("sensitivity late-arrivals scenario found no late feature arrivals".into());
    }

    lags.sort_unstable();
    lags.dedup();
    let steps = steps.max(1);
    let sample_count = lags.len().min(steps);
    let mut policies = Vec::with_capacity(sample_count);
    let mut seen_thresholds = BTreeSet::new();

    for sample_index in 0..sample_count {
        let lag_index = if sample_count == 1 {
            lags.len() - 1
        } else {
            ((lags.len() - 1) * sample_index) / (sample_count - 1)
        };
        let max_lag_inclusive = lags[lag_index];
        if !seen_thresholds.insert(max_lag_inclusive) {
            continue;
        }

        let threshold_pct_bps = if sample_count == 1 {
            10_000
        } else {
            (((sample_index + 1) * 10_000) / sample_count) as u16
        };
        let threshold = if lag_index == lags.len() - 1 {
            None
        } else {
            Some(max_lag_inclusive)
        };
        let name = format!(
            "late_arrivals_cumulative_{}pct",
            format_percent_bps_for_name(threshold_pct_bps)
        );
        policies.push(PolicyPoint::leak_feature_lag_cumulative(
            name,
            threshold,
            threshold_pct_bps,
            10_000,
        ));
    }

    Ok(policies)
}

fn format_percent_bps_for_name(pct_bps: u16) -> String {
    let whole = pct_bps / 100;
    let fractional = pct_bps % 100;
    if fractional == 0 {
        whole.to_string()
    } else if fractional % 10 == 0 {
        format!("{}_{}", whole, fractional / 10)
    } else {
        format!("{}_{fractional:02}", whole)
    }
}

fn shift_features_policy_name(shift: i64) -> String {
    match shift.cmp(&0) {
        std::cmp::Ordering::Less => format!("shift_features_minus_{}", shift.unsigned_abs()),
        std::cmp::Ordering::Equal => "shift_features_0".to_string(),
        std::cmp::Ordering::Greater => format!("shift_features_plus_{shift}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuditArgs {
    events_path: String,
    stored_predictions_path: Option<PathBuf>,
    outcomes_path: Option<PathBuf>,
    signal: SignalChoice,
    out: Option<PathBuf>,
    allow_missing_recipe_hash: bool,
}

fn parse_audit_args(args: &[String]) -> Result<AuditArgs, Box<dyn Error>> {
    let mut positional = Vec::new();
    let mut signal = SignalChoice::default();
    let mut out = None;
    let mut outcomes_path = None;
    let mut allow_missing_recipe_hash = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--signal" => {
                signal = SignalChoice::parse(required_arg(args, index, "--signal")?)?;
                index += 2;
            }
            "--out" => {
                out = Some(PathBuf::from(required_arg(args, index, "--out")?));
                index += 2;
            }
            "--outcomes" => {
                outcomes_path = Some(PathBuf::from(required_arg(args, index, "--outcomes")?));
                index += 2;
            }
            "--allow-missing-recipe-hash" => {
                allow_missing_recipe_hash = true;
                index += 1;
            }
            value if value.starts_with("--") => {
                return Err(format!("unknown audit argument: {value}").into());
            }
            value => {
                positional.push(value.to_string());
                index += 1;
            }
        }
    }

    if positional.len() > 3 {
        return Err("audit accepts at most events, stored predictions, and outcomes paths".into());
    }
    let positional_outcomes_path = positional.get(2).map(PathBuf::from);
    if outcomes_path.is_some() && positional_outcomes_path.is_some() {
        return Err("--outcomes cannot be combined with a positional outcomes path".into());
    }

    Ok(AuditArgs {
        events_path: positional
            .first()
            .cloned()
            .unwrap_or_else(|| "examples/late-arrival.pipe".to_string()),
        stored_predictions_path: positional.get(1).map(PathBuf::from),
        outcomes_path: outcomes_path.or(positional_outcomes_path),
        signal,
        out,
        allow_missing_recipe_hash,
    })
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
        SignalChoice::WindowedZScore => ReplayEngine::with_signal(WindowedZScoreSignal::default())
            .replay_with_order(events, options, order),
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
        SignalChoice::WindowedZScore => run_adversarial_checks_with_options_for_signal(
            events,
            options,
            WindowedZScoreSignal::default(),
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

    let replay_start = Instant::now();
    let replay = replay_with_signal(
        signal,
        &stream.events,
        ReplayOptions::default(),
        ReplayOrder::ReceivedTime,
    )?;
    let replay_elapsed = replay_start.elapsed();
    let report = run_checks_with_signal(signal, &stream.events, CheckOptions::sampled(32));

    let predictions_output = format_prediction_output(&replay);
    let checks_output = format_check_report(&report);
    let summary_output = format_suite_summary(&stream, signal, &replay, &report);
    let manifest = RunManifest::new(RunManifestInputs {
        config: &config,
        stream: &stream,
        signal,
        replay: &replay,
        report: &report,
        events_output: &events_output,
        predictions_output: &predictions_output,
        checks_output: &checks_output,
    });
    let manifest_output = format_run_manifest(&manifest);

    write_file(&predictions_path, &predictions_output)?;
    write_file(&checks_path, &checks_output)?;
    write_file(&summary_path, &summary_output)?;
    write_file(&manifest_path, &manifest_output)?;

    print_run_suite_stdout(RunSuiteStdout {
        out_dir: &out_dir,
        manifest_path: &manifest_path,
        stream: &stream,
        signal,
        replay: &replay,
        replay_elapsed,
        report: &report,
        manifest: &manifest,
    });

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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AuditKey {
    symbol: String,
    prediction_replay_key: String,
}

#[derive(Debug, Clone, Deserialize)]
struct StoredPrediction {
    signal_value: i8,
    feature_recipe_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OutcomeAttribution {
    return_bps: Number,
}

#[derive(Debug, Clone, Serialize)]
struct AuditRecordJson {
    schema_version: u8,
    prediction_id: String,
    signal: String,
    prediction_replay_key: String,
    symbol: String,
    signal_value: i8,
    input_event_ids: Vec<String>,
    max_input_replay_key: Option<String>,
    feature_recipe_hash: String,
    causally_valid: bool,
    matched_stored_prediction: Option<bool>,
    outcome: Option<OutcomeAttribution>,
}

#[derive(Debug, Clone, Deserialize)]
struct StoredPredictionJson {
    prediction_replay_key: String,
    symbol: String,
    signal_value: i8,
    feature_recipe_hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct OutcomeJson {
    prediction_replay_key: String,
    symbol: String,
    return_bps: Number,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuditSummary {
    causally_valid: bool,
    non_causal_records: usize,
    mismatched_stored_predictions: Option<usize>,
    extra_stored_predictions: Option<usize>,
    outcomes_attached: usize,
}

impl AuditSummary {
    fn passed(&self) -> bool {
        self.non_causal_records == 0
            && self.mismatched_stored_predictions.unwrap_or(0) == 0
            && self.extra_stored_predictions.unwrap_or(0) == 0
    }

    fn stored_match_summary(&self) -> String {
        self.mismatched_stored_predictions
            .zip(self.extra_stored_predictions)
            .map(|(mismatched, extra)| (mismatched == 0 && extra == 0).to_string())
            .unwrap_or_else(|| "replay-only".to_string())
    }

    fn failure_message(&self) -> String {
        let mut parts = Vec::new();
        if self.non_causal_records > 0 {
            parts.push(format!("{} non-causal records", self.non_causal_records));
        }
        if let Some(count) = self
            .mismatched_stored_predictions
            .filter(|count| *count > 0)
        {
            parts.push(format!("{count} mismatched or missing stored predictions"));
        }
        if let Some(count) = self.extra_stored_predictions.filter(|count| *count > 0) {
            parts.push(format!("{count} extra stored predictions"));
        }
        if parts.is_empty() {
            "audit failed".to_string()
        } else {
            format!("audit found {}", parts.join(", "))
        }
    }
}

fn format_audit_jsonl(
    signal: SignalChoice,
    output: &ReplayOutput,
    stored_predictions: Option<&BTreeMap<AuditKey, StoredPrediction>>,
    outcomes: &BTreeMap<AuditKey, OutcomeAttribution>,
    allow_missing_recipe_hash: bool,
) -> String {
    let mut text = String::new();
    for record in output.predictions.records() {
        let _ = writeln!(
            text,
            "{}",
            format_audit_record_json(
                signal,
                output,
                record,
                stored_predictions,
                outcomes,
                allow_missing_recipe_hash
            )
        );
    }
    text
}

fn format_audit_record_json(
    signal: SignalChoice,
    output: &ReplayOutput,
    record: &asof_causality_core::PredictionRecord,
    stored_predictions: Option<&BTreeMap<AuditKey, StoredPrediction>>,
    outcomes: &BTreeMap<AuditKey, OutcomeAttribution>,
    allow_missing_recipe_hash: bool,
) -> String {
    let prediction_id = output.predictions.event_label(record.prediction_event_key);
    let prediction_replay_key = output.predictions.format_replay_key(
        record.prediction_time,
        record.prediction_sequence,
        record.prediction_event_key,
    );
    let symbol = output.predictions.symbol_label(record.symbol);
    let input_event_ids = output
        .predictions
        .input_event_labels(record.input_event_ids_used);
    let max_input_replay_key = output.predictions.max_input_replay_key_value(record);
    let feature_recipe_hash = record.feature_recipe_hash_hex();
    let causally_valid = output.predictions.record_is_causal(record);
    let audit_key = AuditKey {
        symbol: symbol.clone(),
        prediction_replay_key: prediction_replay_key.clone(),
    };
    let matched_stored_prediction = stored_predictions.map(|stored| {
        stored.get(&audit_key).is_some_and(|stored_record| {
            stored_record_matches(
                stored_record,
                record.signal_value,
                &feature_recipe_hash,
                allow_missing_recipe_hash,
            )
        })
    });
    let audit_record = AuditRecordJson {
        schema_version: 2,
        prediction_id,
        signal: signal.as_str().to_string(),
        prediction_replay_key,
        symbol,
        signal_value: record.signal_value,
        input_event_ids,
        max_input_replay_key,
        feature_recipe_hash,
        causally_valid,
        matched_stored_prediction,
        outcome: outcomes.get(&audit_key).cloned(),
    };

    serde_json::to_string(&audit_record).expect("audit record should serialize")
}

fn stored_record_matches(
    stored_record: &StoredPrediction,
    signal_value: i8,
    feature_recipe_hash: &str,
    allow_missing_recipe_hash: bool,
) -> bool {
    stored_record.signal_value == signal_value
        && stored_record
            .feature_recipe_hash
            .as_deref()
            .map_or(allow_missing_recipe_hash, |stored_hash| {
                stored_hash == feature_recipe_hash
            })
}

fn summarize_audit(
    output: &ReplayOutput,
    stored_predictions: Option<&BTreeMap<AuditKey, StoredPrediction>>,
    outcomes: &BTreeMap<AuditKey, OutcomeAttribution>,
    allow_missing_recipe_hash: bool,
) -> AuditSummary {
    let non_causal_records = output
        .predictions
        .records()
        .iter()
        .filter(|record| !output.predictions.record_is_causal(record))
        .count();
    let causally_valid = non_causal_records == 0;
    let expected_keys: BTreeSet<AuditKey> = output
        .predictions
        .records()
        .iter()
        .map(|record| {
            let prediction_replay_key = output.predictions.format_replay_key(
                record.prediction_time,
                record.prediction_sequence,
                record.prediction_event_key,
            );
            AuditKey {
                symbol: output.predictions.symbol_label(record.symbol),
                prediction_replay_key,
            }
        })
        .collect();
    let mismatched_stored_predictions = stored_predictions.map(|stored| {
        output
            .predictions
            .records()
            .iter()
            .filter(|record| {
                let prediction_replay_key = output.predictions.format_replay_key(
                    record.prediction_time,
                    record.prediction_sequence,
                    record.prediction_event_key,
                );
                let key = AuditKey {
                    symbol: output.predictions.symbol_label(record.symbol),
                    prediction_replay_key,
                };
                !stored.get(&key).is_some_and(|stored_record| {
                    stored_record_matches(
                        stored_record,
                        record.signal_value,
                        &record.feature_recipe_hash_hex(),
                        allow_missing_recipe_hash,
                    )
                })
            })
            .count()
    });
    let extra_stored_predictions = stored_predictions.map(|stored| {
        stored
            .keys()
            .filter(|key| !expected_keys.contains(*key))
            .count()
    });
    let outcomes_attached = outcomes
        .keys()
        .filter(|key| expected_keys.contains(*key))
        .count();

    AuditSummary {
        causally_valid,
        non_causal_records,
        mismatched_stored_predictions,
        extra_stored_predictions,
        outcomes_attached,
    }
}

struct SensitivityManifestInputs<'a> {
    events_path: &'a str,
    fixture_input: &'a str,
    signal: SignalChoice,
    sweep: &'a SensitivitySweep,
    summary_path: &'a Path,
    summary_jsonl: &'a str,
    sensitivity_curve_svg_path: Option<&'a Path>,
    sensitivity_curve_svg: Option<&'a str>,
    flip_rate_svg_path: &'a Path,
    flip_rate_svg: &'a str,
    input_change_svg_path: &'a Path,
    input_change_svg: &'a str,
    late_arrival_impact_svg_path: Option<&'a Path>,
    late_arrival_impact_svg: Option<&'a str>,
    details_path: Option<&'a Path>,
    details_jsonl: Option<&'a str>,
}

struct SensitivityArtifactPaths<'a> {
    summary: &'a Path,
    sensitivity_curve_svg: Option<&'a Path>,
    flip_rate_svg: &'a Path,
    input_change_svg: &'a Path,
    late_arrival_impact_svg: Option<&'a Path>,
    details: Option<&'a Path>,
    manifest: &'a Path,
}

fn format_sensitivity_summary_jsonl(signal: SignalChoice, sweep: &SensitivitySweep) -> String {
    let mut text = String::new();
    let baseline_row = sensitivity_summary_json(signal, None, &sweep.baseline, None);
    let _ = writeln!(text, "{baseline_row}");

    for result in &sweep.results {
        let row = sensitivity_summary_json(
            signal,
            Some(&sweep.baseline.policy.name),
            &result.run,
            Some(result),
        );
        let _ = writeln!(text, "{row}");
    }

    text
}

fn sensitivity_summary_json(
    signal: SignalChoice,
    vs_baseline: Option<&str>,
    run: &PolicyRun,
    result: Option<&SensitivityPolicyResult>,
) -> String {
    let (
        predictions,
        predictions_with_signal_change,
        predictions_with_recipe_change,
        predictions_with_new_inputs,
        new_input_uses,
        unique_new_inputs,
        flip_rate,
    ) = result
        .map(|result| {
            (
                result.summary.predictions,
                result.summary.predictions_with_signal_change,
                result.summary.predictions_with_recipe_change,
                result.summary.predictions_with_new_inputs,
                result.summary.new_input_uses,
                result.summary.unique_new_inputs,
                result.summary.flip_rate,
            )
        })
        .unwrap_or_else(|| (run.output.predictions.records().len(), 0, 0, 0, 0, 0, 0.0));
    let row = json!({
        "schema_version": 1,
        "policy": policy_json(&run.policy),
        "policy_name": run.policy.name.as_str(),
        "category": run.policy.category.as_str(),
        "vs_baseline": vs_baseline,
        "events_transformed": run.events_transformed,
        "predictions": predictions,
        "predictions_with_signal_change": predictions_with_signal_change,
        "predictions_with_recipe_change": predictions_with_recipe_change,
        "predictions_with_new_inputs": predictions_with_new_inputs,
        "new_input_uses": new_input_uses,
        "unique_new_inputs": unique_new_inputs,
        "flip_rate": flip_rate,
        "transcript_hash": run.output.predictions.transcript_digest(),
        "transformed_fixture_hash": run.transformed_fixture_hash.as_str(),
        "signal_name": signal.as_str(),
    });

    serde_json::to_string(&row).expect("sensitivity summary should serialize")
}

fn format_sensitivity_details_jsonl(sweep: &SensitivitySweep) -> String {
    let mut text = String::new();
    for result in &sweep.results {
        for detail in &result.details {
            let row = sensitivity_detail_json(sweep, result, detail);
            let _ = writeln!(text, "{row}");
        }
    }
    text
}

fn format_sensitivity_flip_rate_svg(sweep: &SensitivitySweep) -> String {
    let rows = sensitivity_chart_rows(sweep);
    let subtitle = if sweep_has_cumulative_late_arrival_policies(sweep) {
        "cumulative flip rate as the late-arrival threshold increases"
    } else if sweep_has_marginal_late_arrival_bucket_policies(sweep) {
        "marginal flip rate: one lag band moved at a time; not cumulative"
    } else {
        "share of predictions whose signal value changed"
    };
    format_bar_chart_svg(
        "Sensitivity Flip Rate",
        subtitle,
        &rows
            .iter()
            .map(|row| ChartRow {
                label: row.label.clone(),
                detail: row.detail.clone(),
                tooltip: row.tooltip.clone(),
                category: row.category.clone(),
                value: row.flip_rate,
                value_label: format!("{:.1}%", row.flip_rate * 100.0),
            })
            .collect::<Vec<_>>(),
        1.0,
    )
}

fn format_sensitivity_input_change_svg(sweep: &SensitivitySweep) -> String {
    let rows = sensitivity_chart_rows(sweep);
    let subtitle = if sweep_has_cumulative_late_arrival_policies(sweep) {
        "cumulative new input-event uses as the late-arrival threshold increases"
    } else if sweep_has_marginal_late_arrival_bucket_policies(sweep) {
        "marginal new input-event uses: one lag band moved at a time"
    } else {
        "new input-event uses admitted by each comparison policy"
    };
    let max_value = rows
        .iter()
        .map(|row| row.new_input_uses as f64)
        .fold(0.0, f64::max)
        .max(1.0);
    format_bar_chart_svg(
        "New Inputs Admitted",
        subtitle,
        &rows
            .iter()
            .map(|row| ChartRow {
                label: row.label.clone(),
                detail: row.detail.clone(),
                tooltip: row.tooltip.clone(),
                category: row.category.clone(),
                value: row.new_input_uses as f64,
                value_label: row.new_input_uses.to_string(),
            })
            .collect::<Vec<_>>(),
        max_value,
    )
}

fn sweep_has_late_arrival_bucket_policies(sweep: &SensitivitySweep) -> bool {
    sweep_has_cumulative_late_arrival_policies(sweep)
        || sweep_has_marginal_late_arrival_bucket_policies(sweep)
}

fn sweep_has_marginal_late_arrival_bucket_policies(sweep: &SensitivitySweep) -> bool {
    sweep
        .results
        .iter()
        .any(|result| is_marginal_late_arrival_bucket_policy(&result.run.policy.kind))
}

fn sweep_has_cumulative_late_arrival_policies(sweep: &SensitivitySweep) -> bool {
    sweep
        .results
        .iter()
        .any(|result| is_cumulative_late_arrival_policy(&result.run.policy.kind))
}

fn is_marginal_late_arrival_bucket_policy(kind: &PolicyKind) -> bool {
    matches!(kind, PolicyKind::ReceivedTimeLagBucketLookahead { .. })
}

fn is_cumulative_late_arrival_policy(kind: &PolicyKind) -> bool {
    matches!(kind, PolicyKind::ReceivedTimeLagCumulativeLookahead { .. })
}

fn format_late_arrival_impact_svg(sweep: &SensitivitySweep) -> String {
    if sweep_has_cumulative_late_arrival_policies(sweep) {
        return format_cumulative_late_arrival_curve_svg(sweep);
    }

    let late_bucket_total = sweep
        .results
        .iter()
        .filter(|result| is_marginal_late_arrival_bucket_policy(&result.run.policy.kind))
        .count();
    let mut late_bucket_index = 0;
    let rows = sweep
        .results
        .iter()
        .filter(|result| is_marginal_late_arrival_bucket_policy(&result.run.policy.kind))
        .map(|result| {
            late_bucket_index += 1;
            let chart_text = policy_chart_text(
                &result.run.policy,
                Some((late_bucket_index, late_bucket_total)),
            );
            ChartRow {
                label: chart_text.label,
                detail: chart_text.detail,
                tooltip: chart_text.tooltip,
                category: result.run.policy.category.as_str().to_string(),
                value: result.summary.flip_rate,
                value_label: format!(
                    "{} ({} changed)",
                    format_percent(result.summary.flip_rate),
                    result.summary.predictions_with_signal_change
                ),
            }
        })
        .collect::<Vec<_>>();

    format_bar_chart_svg(
        "Marginal Late Arrival Impact",
        "one lag band moved at a time; not cumulative",
        &rows,
        1.0,
    )
}

fn format_cumulative_late_arrival_curve_svg(sweep: &SensitivitySweep) -> String {
    #[derive(Debug)]
    struct Point {
        x_bps: u16,
        y: f64,
        label: String,
        tooltip: String,
    }

    let max_new_input_uses = sweep
        .results
        .iter()
        .filter(|result| is_cumulative_late_arrival_policy(&result.run.policy.kind))
        .map(|result| result.summary.new_input_uses)
        .max()
        .unwrap_or(0)
        .max(1);

    let mut points = vec![Point {
        x_bps: 0,
        y: 0.0,
        label: "baseline".to_string(),
        tooltip: "baseline: no late arrivals moved".to_string(),
    }];

    for result in &sweep.results {
        if let PolicyKind::ReceivedTimeLagCumulativeLookahead {
            max_lag_inclusive,
            threshold_pct_bps,
            ..
        } = result.run.policy.kind
        {
            let threshold_label = max_lag_inclusive
                .map(|lag| format!("raw lag <= {}", format_u64_grouped(lag)))
                .unwrap_or_else(|| "all late arrivals".to_string());
            points.push(Point {
                x_bps: threshold_pct_bps,
                y: result.summary.new_input_uses as f64 / max_new_input_uses as f64,
                label: format_percent_bps_for_display(threshold_pct_bps),
                tooltip: format!(
                    "{}: {}; {} new input uses; {} changed; {} flip",
                    format_percent_bps_for_display(threshold_pct_bps),
                    threshold_label,
                    result.summary.new_input_uses,
                    result.summary.predictions_with_signal_change,
                    format_percent(result.summary.flip_rate)
                ),
            });
        }
    }

    points.sort_by_key(|point| point.x_bps);
    let y_max = 1.0_f64;

    let width = 900_f64;
    let height = 520_f64;
    let left = 82_f64;
    let right = 44_f64;
    let top = 126_f64;
    let bottom = 94_f64;
    let plot_width = width - left - right;
    let plot_height = height - top - bottom;
    let x_to_px = |x_bps: u16| left + (f64::from(x_bps) / 10_000.0) * plot_width;
    let y_to_px = |y: f64| top + ((y_max - y) / y_max) * plot_height;

    let mut svg = String::new();
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" role="img" aria-labelledby="title desc" viewBox="0 0 {width:.0} {height:.0}">"#
    );
    let _ = writeln!(
        svg,
        "<title id=\"title\">Cumulative Late Arrival Exposure</title><desc id=\"desc\">x is cumulative late-arrival lag threshold; y is cumulative new input exposure</desc>"
    );
    svg.push_str(
        r##"<rect width="100%" height="100%" fill="#ffffff"/>
<style>
  text { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; fill: #1f2937; }
  .title { font-size: 22px; font-weight: 700; }
  .subtitle { font-size: 13px; fill: #64748b; }
  .note { font-size: 11px; fill: #475569; }
  .axis-label { font-size: 12px; font-weight: 650; fill: #334155; }
  .tick { font-size: 11px; fill: #64748b; }
  .grid { stroke: #e2e8f0; stroke-width: 1; }
  .axis { stroke: #94a3b8; stroke-width: 1.25; }
  .curve { fill: none; stroke: #2563eb; stroke-width: 2.5; stroke-linejoin: round; stroke-linecap: round; }
  .point { fill: #2563eb; stroke: #ffffff; stroke-width: 1.5; }
  .point-label { font-size: 10px; fill: #334155; }
</style>
"##,
    );
    let _ = writeln!(
        svg,
        r#"<text x="28" y="34" class="title">Cumulative Late Arrival Exposure</text>"#
    );
    let _ = writeln!(
        svg,
        r#"<text x="28" y="56" class="subtitle">x = cumulative late-arrival threshold; y = new input uses admitted</text>"#
    );
    let _ = writeln!(
        svg,
        r#"<text x="28" y="78" class="note">Each point replays all late feature/correction events up to that threshold.</text>"#
    );

    for tick in 0..=4 {
        let y_value = y_max * f64::from(tick) / 4.0;
        let y = y_to_px(y_value);
        let _ = writeln!(
            svg,
            r#"<line x1="{left:.2}" y1="{y:.2}" x2="{:.2}" y2="{y:.2}" class="grid"/>"#,
            width - right
        );
        let _ = writeln!(
            svg,
            r#"<text x="{:.2}" y="{:.2}" class="tick" text-anchor="end">{}</text>"#,
            left - 10.0,
            y + 4.0,
            format_percent(y_value)
        );
    }

    for tick in [0_u16, 2500, 5000, 7500, 10000] {
        let x = x_to_px(tick);
        let _ = writeln!(
            svg,
            r#"<line x1="{x:.2}" y1="{top:.2}" x2="{x:.2}" y2="{:.2}" class="grid"/>"#,
            height - bottom
        );
        let _ = writeln!(
            svg,
            r#"<text x="{x:.2}" y="{:.2}" class="tick" text-anchor="middle">{}</text>"#,
            height - bottom + 24.0,
            format_percent_bps_for_display(tick)
        );
    }

    let _ = writeln!(
        svg,
        r#"<line x1="{left:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" class="axis"/>"#,
        height - bottom,
        width - right,
        height - bottom
    );
    let _ = writeln!(
        svg,
        r#"<line x1="{left:.2}" y1="{top:.2}" x2="{left:.2}" y2="{:.2}" class="axis"/>"#,
        height - bottom
    );

    let polyline = points
        .iter()
        .map(|point| format!("{:.2},{:.2}", x_to_px(point.x_bps), y_to_px(point.y)))
        .collect::<Vec<_>>()
        .join(" ");
    let _ = writeln!(svg, r#"<polyline points="{polyline}" class="curve"/>"#);

    for (index, point) in points.iter().enumerate() {
        let x = x_to_px(point.x_bps);
        let y = y_to_px(point.y);
        let _ = writeln!(svg, "<g>");
        let _ = writeln!(svg, "<title>{}</title>", xml_escape(&point.tooltip));
        let _ = writeln!(
            svg,
            r#"<circle cx="{x:.2}" cy="{y:.2}" r="4" class="point"/>"#
        );
        let previous_y = index
            .checked_sub(1)
            .and_then(|previous_index| points.get(previous_index))
            .map(|previous| previous.y)
            .unwrap_or(point.y);
        let exposure_changed = index == 0 || (point.y - previous_y).abs() > 0.0001;
        if point.x_bps == 0 || point.x_bps == 10_000 || exposure_changed {
            let _ = writeln!(
                svg,
                r#"<text x="{:.2}" y="{:.2}" class="point-label" text-anchor="middle">{} {}</text>"#,
                x,
                y - 10.0,
                xml_escape(&point.label),
                format_percent(point.y)
            );
        }
        let _ = writeln!(svg, "</g>");
    }

    let _ = writeln!(
        svg,
        r#"<text x="{:.2}" y="{:.2}" class="axis-label" text-anchor="middle">cumulative late-arrival threshold</text>"#,
        left + plot_width / 2.0,
        height - 20.0
    );
    let _ = writeln!(
        svg,
        r#"<text x="18" y="{:.2}" class="axis-label" transform="rotate(-90 18 {:.2})" text-anchor="middle">new input exposure</text>"#,
        top + plot_height / 2.0,
        top + plot_height / 2.0
    );
    svg.push_str("</svg>\n");
    svg
}

fn sweep_has_sensitivity_curve_points(sweep: &SensitivitySweep) -> bool {
    sweep.results.iter().any(|result| {
        matches!(
            result.run.policy.kind,
            PolicyKind::ReceivedTimeLagFraction { .. } | PolicyKind::ReceivedTimeShift { .. }
        )
    })
}

fn format_sensitivity_curve_svg(sweep: &SensitivitySweep) -> String {
    let uses_lag_fraction = sweep.results.iter().any(|result| {
        matches!(
            result.run.policy.kind,
            PolicyKind::ReceivedTimeLagFraction { .. }
        )
    });
    let mut points = Vec::new();
    points.push(SensitivityCurvePoint {
        label: sweep.baseline.policy.name.clone(),
        category: sweep.baseline.policy.category.as_str().to_string(),
        x: 0.0,
        y: 0.0,
    });

    for result in &sweep.results {
        match &result.run.policy.kind {
            PolicyKind::ReceivedTimeLagFraction { pct_bps, .. } => {
                points.push(SensitivityCurvePoint {
                    label: result.run.policy.name.clone(),
                    category: result.run.policy.category.as_str().to_string(),
                    x: f64::from(*pct_bps) / 100.0,
                    y: result.summary.flip_rate,
                });
            }
            PolicyKind::ReceivedTimeShift { shift, .. } if !uses_lag_fraction => {
                points.push(SensitivityCurvePoint {
                    label: result.run.policy.name.clone(),
                    category: result.run.policy.category.as_str().to_string(),
                    x: -(*shift as f64),
                    y: result.summary.flip_rate,
                });
            }
            _ => {}
        }
    }

    points.sort_by(|left, right| {
        left.x
            .total_cmp(&right.x)
            .then_with(|| left.label.cmp(&right.label))
    });

    let leaky_endpoint = sweep.results.iter().find(|result| {
        matches!(
            result.run.policy.kind,
            PolicyKind::ReplayOrderOverride {
                order: ReplayOrder::ObservedTimeLeaky
            }
        )
    });
    let leaky_flip_rate = leaky_endpoint.map(|result| result.summary.flip_rate);
    let first_effect = points
        .iter()
        .filter(|point| point.label != sweep.baseline.policy.name)
        .find(|point| point.y > 0.0);

    let sampled_offsets = summarize_sampled_offsets(&points, uses_lag_fraction);
    let first_effect_label = first_effect
        .map(|point| {
            format!(
                "first sampled effect: x={} via {} at {}",
                format_curve_x_value(point.x, uses_lag_fraction),
                point.label,
                format_percent(point.y)
            )
        })
        .unwrap_or_else(|| "first sampled effect: not observed in curve samples".into());
    let endpoint_label = leaky_endpoint.map(|result| {
        format!(
            "{} reference: {} flip, {} new input uses",
            result.run.policy.name,
            format_percent(result.summary.flip_rate),
            result.summary.new_input_uses
        )
    });
    let subtitle = if uses_lag_fraction {
        "x = percent of each feature lag removed; y = flip rate"
    } else {
        "x = lookahead stress (-received_time_shift) in fixture-native integer units; y = flip rate"
    };
    let x_axis_label = if uses_lag_fraction {
        "feature publication lag removed (%)"
    } else {
        "lookahead stress (-shift, fixture-native units)"
    };

    let width = 860_f64;
    let height = 520_f64;
    let left = 92_f64;
    let right = 42_f64;
    let top = 126_f64;
    let bottom = 104_f64;
    let plot_width = width - left - right;
    let plot_height = height - top - bottom;

    let (mut x_min, mut x_max) = points.iter().fold((0.0_f64, 0.0_f64), |(min, max), point| {
        (min.min(point.x), max.max(point.x))
    });
    if (x_max - x_min).abs() < f64::EPSILON {
        x_min -= 1.0;
        x_max += 1.0;
    }

    let observed_y_max = points
        .iter()
        .map(|point| point.y)
        .chain(leaky_flip_rate)
        .fold(0.0_f64, f64::max);
    let y_max = nice_flip_axis_max(observed_y_max);

    let x_to_px = |x: f64| left + ((x - x_min) / (x_max - x_min)) * plot_width;
    let y_to_px = |y: f64| top + ((y_max - y) / y_max) * plot_height;

    let mut svg = String::new();
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" role="img" aria-labelledby="title desc" viewBox="0 0 {width:.0} {height:.0}">"#
    );
    let _ = writeln!(
        svg,
        "<title id=\"title\">Sensitivity Curve</title><desc id=\"desc\">{}</desc>",
        xml_escape(subtitle)
    );
    svg.push_str(
        r##"<rect width="100%" height="100%" fill="#ffffff"/>
<style>
  text { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; fill: #1f2937; }
  .title { font-size: 22px; font-weight: 700; }
  .subtitle { font-size: 13px; fill: #64748b; }
  .note { font-size: 12px; fill: #475569; }
  .axis-label { font-size: 12px; font-weight: 650; fill: #334155; }
  .tick { font-size: 11px; fill: #64748b; }
  .grid { stroke: #e2e8f0; stroke-width: 1; }
  .axis { stroke: #94a3b8; stroke-width: 1.25; }
  .curve { fill: none; stroke: #2563eb; stroke-width: 2.5; stroke-linejoin: round; stroke-linecap: round; }
  .endpoint { stroke: #dc2626; stroke-width: 1.5; stroke-dasharray: 5 5; }
  .point-label { font-size: 11px; fill: #334155; }
</style>
"##,
    );
    let _ = writeln!(
        svg,
        r#"<text x="28" y="34" class="title">Sensitivity Curve</text>"#
    );
    let _ = writeln!(
        svg,
        r#"<text x="28" y="56" class="subtitle">{}</text>"#,
        xml_escape(subtitle)
    );
    let _ = writeln!(
        svg,
        r#"<text x="28" y="78" class="note">sampled x-values: {}; no interpolation between points</text>"#,
        xml_escape(&sampled_offsets)
    );
    let _ = writeln!(
        svg,
        r#"<text x="28" y="98" class="note">{}</text>"#,
        xml_escape(&first_effect_label)
    );
    if let Some(endpoint_label) = endpoint_label {
        let _ = writeln!(
            svg,
            r#"<text x="520" y="98" class="note">{}</text>"#,
            xml_escape(&endpoint_label)
        );
    }

    for step in 0..=4 {
        let ratio = step as f64 / 4.0;
        let y_value = y_max * ratio;
        let y = y_to_px(y_value);
        let _ = writeln!(
            svg,
            r#"<line x1="{left:.0}" y1="{y:.2}" x2="{:.0}" y2="{y:.2}" class="grid"/>"#,
            left + plot_width
        );
        let _ = writeln!(
            svg,
            r#"<text x="{:.0}" y="{:.2}" class="tick" text-anchor="end">{}</text>"#,
            left - 10.0,
            y + 4.0,
            xml_escape(&format_percent(y_value))
        );
    }

    let x_tick_values = curve_x_ticks(&points, x_min, x_max);
    for x_value in x_tick_values {
        let x = x_to_px(x_value);
        let _ = writeln!(
            svg,
            r#"<line x1="{x:.2}" y1="{top:.0}" x2="{x:.2}" y2="{:.0}" class="grid"/>"#,
            top + plot_height
        );
        let _ = writeln!(
            svg,
            r#"<text x="{x:.2}" y="{:.0}" class="tick" text-anchor="middle">{}</text>"#,
            top + plot_height + 22.0,
            xml_escape(&format_curve_x_value(x_value, uses_lag_fraction))
        );
    }

    let axis_bottom = top + plot_height;
    let axis_right = left + plot_width;
    let _ = writeln!(
        svg,
        r#"<line x1="{left:.0}" y1="{top:.0}" x2="{left:.0}" y2="{axis_bottom:.0}" class="axis"/>"#
    );
    let _ = writeln!(
        svg,
        r#"<line x1="{left:.0}" y1="{axis_bottom:.0}" x2="{axis_right:.0}" y2="{axis_bottom:.0}" class="axis"/>"#
    );
    let _ = writeln!(
        svg,
        r#"<text x="{:.0}" y="{}" class="axis-label" text-anchor="middle">{}</text>"#,
        left + plot_width / 2.0,
        height - 26.0,
        xml_escape(x_axis_label)
    );
    let _ = writeln!(
        svg,
        r#"<text x="22" y="{:.0}" class="axis-label" transform="rotate(-90 22 {:.0})" text-anchor="middle">flip rate</text>"#,
        top + plot_height / 2.0,
        top + plot_height / 2.0
    );

    if let Some(endpoint_y) = leaky_flip_rate {
        let y = y_to_px(endpoint_y);
        let _ = writeln!(
            svg,
            r#"<line x1="{left:.0}" y1="{y:.2}" x2="{axis_right:.0}" y2="{y:.2}" class="endpoint"/>"#
        );
        let _ = writeln!(
            svg,
            r#"<text x="{:.0}" y="{:.2}" class="point-label" text-anchor="start">observed-time policy reference</text>"#,
            left + 8.0,
            y - 8.0
        );
    }

    if points.len() > 1 {
        let polyline = points
            .iter()
            .map(|point| format!("{:.2},{:.2}", x_to_px(point.x), y_to_px(point.y)))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(svg, r#"<polyline points="{polyline}" class="curve"/>"#);
    }

    for point in &points {
        let x = x_to_px(point.x);
        let y = y_to_px(point.y);
        let fill = category_color(&point.category);
        let label = if point.label == sweep.baseline.policy.name {
            "baseline".to_string()
        } else {
            format!(
                "{} {}",
                format_curve_x_value(point.x, uses_lag_fraction),
                format_percent(point.y)
            )
        };
        let _ = writeln!(
            svg,
            r##"<circle cx="{x:.2}" cy="{y:.2}" r="5" fill="{fill}" stroke="#ffffff" stroke-width="2"/>"##
        );
        let _ = writeln!(
            svg,
            r#"<text x="{x:.2}" y="{:.2}" class="point-label" text-anchor="middle">{}</text>"#,
            y - 12.0,
            xml_escape(&label)
        );
    }

    svg.push_str("</svg>\n");
    svg
}

#[derive(Debug, Clone, PartialEq)]
struct SensitivityCurvePoint {
    label: String,
    category: String,
    x: f64,
    y: f64,
}

fn nice_flip_axis_max(value: f64) -> f64 {
    if value <= 0.01 {
        0.01
    } else if value <= 0.05 {
        0.05
    } else if value <= 0.10 {
        0.10
    } else if value <= 0.25 {
        0.25
    } else if value <= 0.50 {
        0.50
    } else {
        1.0
    }
}

fn summarize_sampled_offsets(points: &[SensitivityCurvePoint], as_percent: bool) -> String {
    let mut labels = points
        .iter()
        .map(|point| format_curve_x_value(point.x, as_percent))
        .collect::<Vec<_>>();
    labels.dedup();
    if labels.len() <= 8 {
        labels.join(", ")
    } else {
        format!(
            "{} explicit samples from {} to {}",
            labels.len(),
            labels.first().expect("offset labels should not be empty"),
            labels.last().expect("offset labels should not be empty")
        )
    }
}

fn curve_x_ticks(points: &[SensitivityCurvePoint], x_min: f64, x_max: f64) -> Vec<f64> {
    let mut sampled = points.iter().map(|point| point.x).collect::<Vec<_>>();
    sampled.dedup_by(|left, right| (*left - *right).abs() < f64::EPSILON);
    if sampled.len() <= 6 {
        return sampled;
    }

    (0..=4)
        .map(|step| x_min + (x_max - x_min) * step as f64 / 4.0)
        .collect()
}

fn format_axis_number(value: f64) -> String {
    if (value - value.round()).abs() < f64::EPSILON {
        format!("{:.0}", value)
    } else {
        format!("{value:.2}")
    }
}

fn format_curve_x_value(value: f64, as_percent: bool) -> String {
    if as_percent {
        format!("{}%", format_axis_number(value))
    } else {
        format_axis_number(value)
    }
}

fn format_percent(value: f64) -> String {
    format!("{:.1}%", value * 100.0)
}

#[derive(Debug, Clone, PartialEq)]
struct SensitivityChartRow {
    label: String,
    detail: String,
    tooltip: String,
    category: String,
    flip_rate: f64,
    new_input_uses: usize,
}

fn sensitivity_chart_rows(sweep: &SensitivitySweep) -> Vec<SensitivityChartRow> {
    let late_bucket_total = sweep
        .results
        .iter()
        .filter(|result| is_marginal_late_arrival_bucket_policy(&result.run.policy.kind))
        .count();
    let late_cumulative_total = sweep
        .results
        .iter()
        .filter(|result| is_cumulative_late_arrival_policy(&result.run.policy.kind))
        .count();
    let mut late_bucket_index = 0;
    let mut late_cumulative_index = 0;
    let mut rows = Vec::with_capacity(sweep.results.len());

    for result in &sweep.results {
        let late_bucket_position =
            if is_marginal_late_arrival_bucket_policy(&result.run.policy.kind) {
                late_bucket_index += 1;
                Some((late_bucket_index, late_bucket_total))
            } else if is_cumulative_late_arrival_policy(&result.run.policy.kind) {
                late_cumulative_index += 1;
                Some((late_cumulative_index, late_cumulative_total))
            } else {
                None
            };
        let chart_text = policy_chart_text(&result.run.policy, late_bucket_position);
        rows.push(SensitivityChartRow {
            label: chart_text.label,
            detail: chart_text.detail,
            tooltip: chart_text.tooltip,
            category: result.run.policy.category.as_str().to_string(),
            flip_rate: result.summary.flip_rate,
            new_input_uses: result.summary.new_input_uses,
        });
    }

    rows
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PolicyChartText {
    label: String,
    detail: String,
    tooltip: String,
}

fn policy_chart_text(
    policy: &PolicyPoint,
    late_bucket_position: Option<(usize, usize)>,
) -> PolicyChartText {
    match &policy.kind {
        PolicyKind::ReceivedTimeLagBucketLookahead {
            min_lag,
            max_lag_exclusive,
            ..
        } => {
            let (index, total) = late_bucket_position.unwrap_or((1, 1));
            let label = late_arrival_bucket_display_label(index, total);
            PolicyChartText {
                label: label.clone(),
                detail: format!("marginal bucket {index} of {total}; not cumulative"),
                tooltip: format!(
                    "{label}: raw fixture lag {}",
                    format_lag_range(*min_lag, *max_lag_exclusive)
                ),
            }
        }
        PolicyKind::ReceivedTimeLagCumulativeLookahead {
            max_lag_inclusive,
            threshold_pct_bps,
            ..
        } => {
            let (index, total) = late_bucket_position.unwrap_or((1, 1));
            let label = if *threshold_pct_bps == 10_000 {
                "All late arrivals included".to_string()
            } else {
                format!(
                    "Up to {} lateness threshold",
                    format_percent_bps_for_display(*threshold_pct_bps)
                )
            };
            let threshold = max_lag_inclusive
                .map(|lag| format!("raw lag <= {}", format_u64_grouped(lag)))
                .unwrap_or_else(|| "all late arrivals".to_string());
            PolicyChartText {
                label: label.clone(),
                detail: format!("cumulative sample {index} of {total}"),
                tooltip: format!("{label}: {threshold}"),
            }
        }
        PolicyKind::ReceivedTimeLagFraction { pct_bps, .. } => {
            let label = format!("{} lag removed", format_percent_bps_for_display(*pct_bps));
            let detail = "synthetic stress; bounded by each event's observed_time".to_string();
            PolicyChartText {
                label: label.clone(),
                detail: detail.clone(),
                tooltip: format!("{label}: {detail}"),
            }
        }
        PolicyKind::ReceivedTimeShift { shift, .. } => {
            let label = format!("Feature timestamp shift {shift:+}");
            let detail = "synthetic stress; fixture-native integer units".to_string();
            PolicyChartText {
                label: label.clone(),
                detail: detail.clone(),
                tooltip: format!("{label}: {detail}"),
            }
        }
        PolicyKind::ReplayOrderOverride {
            order: ReplayOrder::ObservedTimeLeaky,
        } => {
            let label = "Observed-time replay".to_string();
            let detail = "realistic failure reference".to_string();
            PolicyChartText {
                label: label.clone(),
                detail: detail.clone(),
                tooltip: format!("{label}: {detail}"),
            }
        }
        PolicyKind::ReplayOrderOverride { .. } => {
            let label = policy.name.clone();
            let detail = humanize_category(policy.category.as_str()).to_string();
            PolicyChartText {
                label: label.clone(),
                detail: detail.clone(),
                tooltip: format!("{label}: {detail}"),
            }
        }
        PolicyKind::Identity => {
            let label = "Strict received-time baseline".to_string();
            let detail = humanize_category(policy.category.as_str()).to_string();
            PolicyChartText {
                label: label.clone(),
                detail: detail.clone(),
                tooltip: format!("{label}: {detail}"),
            }
        }
    }
}

fn late_arrival_bucket_display_label(index: usize, total: usize) -> String {
    match (index, total) {
        (_, 0 | 1) => "Late arrivals".to_string(),
        (1, _) => "Shortest late arrivals".to_string(),
        (index, total) if index == total => "Longest late arrivals".to_string(),
        (index, total) => format!("Late-arrival bucket {index} of {total}"),
    }
}

fn format_lag_range(min_lag: u64, max_lag_exclusive: Option<u64>) -> String {
    match max_lag_exclusive {
        Some(max) => format!(
            "[{}, {})",
            format_u64_grouped(min_lag),
            format_u64_grouped(max)
        ),
        None => format!(">= {}", format_u64_grouped(min_lag)),
    }
}

fn format_u64_grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::new();
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped.chars().rev().collect()
}

fn format_percent_bps_for_display(pct_bps: u16) -> String {
    let value = f64::from(pct_bps) / 100.0;
    if pct_bps % 100 == 0 {
        format!("{value:.0}%")
    } else {
        format!("{value:.2}%")
    }
}

fn humanize_category(category: &str) -> &'static str {
    match category {
        "baseline" => "baseline",
        "synthetic_stress" => "synthetic stress",
        "realistic_failure" => "realistic failure",
        _ => "comparison policy",
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ChartRow {
    label: String,
    detail: String,
    tooltip: String,
    category: String,
    value: f64,
    value_label: String,
}

fn format_bar_chart_svg(title: &str, subtitle: &str, rows: &[ChartRow], max_value: f64) -> String {
    let row_height = 52_usize;
    let top = 84_usize;
    let bottom = 42_usize;
    let left = 300_usize;
    let chart_width = 470_f64;
    let width = 900_usize;
    let height = top + bottom + rows.len().max(1) * row_height;
    let mut svg = String::new();

    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" role="img" aria-labelledby="title desc" viewBox="0 0 {width} {height}">"#
    );
    let _ = writeln!(svg, "<title id=\"title\">{}</title>", xml_escape(title));
    let _ = writeln!(svg, "<desc id=\"desc\">{}</desc>", xml_escape(subtitle));
    svg.push_str(
        r##"<rect width="100%" height="100%" fill="#ffffff"/>
<style>
  text { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; fill: #1f2937; }
  .title { font-size: 22px; font-weight: 700; }
  .subtitle { font-size: 13px; fill: #64748b; }
  .label { font-size: 12px; font-weight: 650; }
  .detail { font-size: 10px; fill: #64748b; }
  .value { font-size: 12px; font-weight: 700; }
  .axis { stroke: #cbd5e1; stroke-width: 1; }
</style>
"##,
    );
    let _ = writeln!(
        svg,
        r#"<text x="28" y="34" class="title">{}</text>"#,
        xml_escape(title)
    );
    let _ = writeln!(
        svg,
        r#"<text x="28" y="56" class="subtitle">{}</text>"#,
        xml_escape(subtitle)
    );
    let axis_y = height - bottom + 2;
    let _ = writeln!(
        svg,
        r#"<line x1="{left}" y1="{axis_y}" x2="{}" y2="{axis_y}" class="axis"/>"#,
        left as f64 + chart_width
    );

    if rows.is_empty() {
        let _ = writeln!(
            svg,
            r#"<text x="28" y="{top}" class="subtitle">No comparison policies were emitted.</text>"#
        );
    }

    for (index, row) in rows.iter().enumerate() {
        let y = top + index * row_height;
        let bar_y = y + 13;
        let label_y = y + 16;
        let detail_y = y + 34;
        let normalized = if max_value <= 0.0 {
            0.0
        } else {
            (row.value / max_value).clamp(0.0, 1.0)
        };
        let bar_width = (normalized * chart_width).max(if row.value > 0.0 { 2.0 } else { 0.0 });
        let fill = category_color(&row.category);
        let value_x = left as f64 + bar_width + 10.0;

        let _ = writeln!(svg, "<g>");
        let _ = writeln!(svg, "<title>{}</title>", xml_escape(&row.tooltip));
        let _ = writeln!(
            svg,
            r#"<text x="28" y="{label_y}" class="label">{}</text>"#,
            xml_escape(&row.label)
        );
        let _ = writeln!(
            svg,
            r#"<text x="28" y="{detail_y}" class="detail">{}</text>"#,
            xml_escape(&row.detail)
        );
        let _ = writeln!(
            svg,
            r#"<rect x="{left}" y="{bar_y}" width="{bar_width:.2}" height="18" rx="3" fill="{fill}"/>"#
        );
        let _ = writeln!(
            svg,
            r#"<text x="{value_x:.2}" y="{}" class="value">{}</text>"#,
            bar_y + 14,
            xml_escape(&row.value_label)
        );
        let _ = writeln!(svg, "</g>");
    }

    svg.push_str("</svg>\n");
    svg
}

fn category_color(category: &str) -> &'static str {
    match category {
        "realistic_failure" => "#dc2626",
        "synthetic_stress" => "#2563eb",
        _ => "#475569",
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn sensitivity_detail_json(
    sweep: &SensitivitySweep,
    result: &SensitivityPolicyResult,
    detail: &asof_causality_core::SensitivityDetail,
) -> String {
    let baseline_output = &sweep.baseline.output;
    let comparison_output = &result.run.output;
    let row = json!({
        "schema_version": 1,
        "policy_name": result.run.policy.name.as_str(),
        "category": result.run.policy.category.as_str(),
        "prediction_event_id": baseline_output.predictions.event_label(detail.prediction_event_key),
        "baseline_prediction_replay_key": baseline_output.predictions.format_replay_key(
            detail.baseline.prediction_time,
            detail.baseline.prediction_sequence,
            detail.baseline.prediction_event_key,
        ),
        "comparison_prediction_replay_key": comparison_output.predictions.format_replay_key(
            detail.comparison.prediction_time,
            detail.comparison.prediction_sequence,
            detail.comparison.prediction_event_key,
        ),
        "prediction_time_baseline": detail.baseline.prediction_time,
        "prediction_time_comparison": detail.comparison.prediction_time,
        "baseline": prediction_record_sensitivity_json(baseline_output, &detail.baseline),
        "comparison": prediction_record_sensitivity_json(comparison_output, &detail.comparison),
        "new_inputs_admitted": detail
            .new_inputs_admitted
            .iter()
            .map(|event_key| comparison_output.predictions.event_label(*event_key))
            .collect::<Vec<_>>(),
        "signal_value_changed": detail.signal_value_changed,
        "feature_recipe_hash_changed": detail.feature_recipe_hash_changed,
    });

    serde_json::to_string(&row).expect("sensitivity detail should serialize")
}

fn prediction_record_sensitivity_json(
    output: &ReplayOutput,
    record: &asof_causality_core::PredictionRecord,
) -> Value {
    json!({
        "signal_value": record.signal_value,
        "input_event_ids_used": output.predictions.input_event_labels(record.input_event_ids_used),
        "feature_recipe_hash": record.feature_recipe_hash_hex(),
    })
}

fn format_sensitivity_manifest_json(inputs: SensitivityManifestInputs<'_>) -> String {
    let invocation_args = env::args().collect::<Vec<_>>();
    let details_path = inputs.details_path.map(|path| path.display().to_string());
    let details_hash = inputs
        .details_jsonl
        .map(|details| asof_causality_core::blake3_hex(details.as_bytes()));
    let sensitivity_curve_svg_path = inputs
        .sensitivity_curve_svg_path
        .map(|path| path.display().to_string());
    let sensitivity_curve_svg_hash = inputs
        .sensitivity_curve_svg
        .map(|svg| asof_causality_core::blake3_hex(svg.as_bytes()));
    let late_arrival_impact_svg_path = inputs
        .late_arrival_impact_svg_path
        .map(|path| path.display().to_string());
    let late_arrival_impact_svg_hash = inputs
        .late_arrival_impact_svg
        .map(|svg| asof_causality_core::blake3_hex(svg.as_bytes()));
    let policies = std::iter::once(&inputs.sweep.baseline)
        .chain(inputs.sweep.results.iter().map(|result| &result.run))
        .map(policy_run_manifest_json)
        .collect::<Vec<_>>();
    let manifest = json!({
        "schema_version": "sensitivity-v1",
        "tool": "asof-causality",
        "hash_algorithm": "blake3",
        "run_started_utc": system_time_to_utc_iso8601(SystemTime::now()),
        "invocation": shell_join(&invocation_args),
        "invocation_args": invocation_args,
        "source_commit": current_git_commit(),
        "workspace_dirty": current_workspace_dirty(),
        "rust_toolchain": current_rustc_version(),
        "timestamp_shift_time_axis": "fixture_native_integer",
        "timestamp_shift_semantics": "raw signed integer arithmetic on received_time; no calendar validation",
        "fixture_path": inputs.events_path,
        "fixture_hash": asof_causality_core::blake3_hex(inputs.fixture_input.as_bytes()),
        "signal": inputs.signal.as_str(),
        "signal_config_descriptor": inputs.signal.config_descriptor(),
        "baseline_policy": inputs.sweep.baseline.policy.name.as_str(),
        "baseline_transcript_hash": inputs.sweep.baseline.output.predictions.transcript_digest(),
        "policies": policies,
        "summary_path": inputs.summary_path.display().to_string(),
        "summary_hash": asof_causality_core::blake3_hex(inputs.summary_jsonl.as_bytes()),
        "sensitivity_curve_svg_path": sensitivity_curve_svg_path,
        "sensitivity_curve_svg_hash": sensitivity_curve_svg_hash,
        "flip_rate_svg_path": inputs.flip_rate_svg_path.display().to_string(),
        "flip_rate_svg_hash": asof_causality_core::blake3_hex(inputs.flip_rate_svg.as_bytes()),
        "input_change_svg_path": inputs.input_change_svg_path.display().to_string(),
        "input_change_svg_hash": asof_causality_core::blake3_hex(inputs.input_change_svg.as_bytes()),
        "late_arrival_impact_svg_path": late_arrival_impact_svg_path,
        "late_arrival_impact_svg_hash": late_arrival_impact_svg_hash,
        "details_path": details_path,
        "details_hash": details_hash,
    });

    format!(
        "{}\n",
        serde_json::to_string_pretty(&manifest).expect("sensitivity manifest should serialize")
    )
}

fn policy_run_manifest_json(run: &PolicyRun) -> Value {
    json!({
        "name": run.policy.name.as_str(),
        "category": run.policy.category.as_str(),
        "descriptor": policy_json(&run.policy),
        "events_transformed": run.events_transformed,
        "transformed_fixture_hash": run.transformed_fixture_hash.as_str(),
        "transcript_hash": run.output.predictions.transcript_digest(),
    })
}

fn policy_json(policy: &PolicyPoint) -> Value {
    match &policy.kind {
        PolicyKind::Identity => json!({
            "name": policy.name.as_str(),
            "category": policy.category.as_str(),
            "kind": "identity",
        }),
        PolicyKind::ReceivedTimeShift {
            roles_affected,
            shift,
        } => json!({
            "name": policy.name.as_str(),
            "category": policy.category.as_str(),
            "kind": "received_time_shift",
            "time_axis": "fixture_native_integer",
            "shift_units": "fixture_native_integer",
            "calendar_aware": false,
            "roles_affected": roles_affected
                .iter()
                .map(|role| role.as_str())
                .collect::<Vec<_>>(),
            "shift": shift,
            "preserve": ["event_id", "observed_time", "sequence", "symbol", "payload"],
        }),
        PolicyKind::ReceivedTimeLagFraction {
            roles_affected,
            pct_bps,
        } => json!({
            "name": policy.name.as_str(),
            "category": policy.category.as_str(),
            "kind": "received_time_lag_fraction",
            "time_axis": "event_lag_fraction_integer",
            "shift_units": "percent_of_each_event_lag",
            "calendar_aware": false,
            "bounded_by_observed_time": true,
            "roles_affected": roles_affected
                .iter()
                .map(|role| role.as_str())
                .collect::<Vec<_>>(),
            "lag_fraction_bps": pct_bps,
            "lag_fraction_percent": f64::from(*pct_bps) / 100.0,
            "preserve": ["event_id", "observed_time", "sequence", "symbol", "payload"],
        }),
        PolicyKind::ReceivedTimeLagBucketLookahead {
            roles_affected,
            min_lag,
            max_lag_exclusive,
            pct_bps,
        } => json!({
            "name": policy.name.as_str(),
            "category": policy.category.as_str(),
            "kind": "received_time_lag_bucket_lookahead",
            "time_axis": "event_lag_fixture_native_integer",
            "shift_units": "percent_of_each_event_lag",
            "calendar_aware": false,
            "bounded_by_observed_time": true,
            "roles_affected": roles_affected
                .iter()
                .map(|role| role.as_str())
                .collect::<Vec<_>>(),
            "min_lag": min_lag,
            "max_lag_exclusive": max_lag_exclusive,
            "lag_fraction_bps": pct_bps,
            "lag_fraction_percent": f64::from(*pct_bps) / 100.0,
            "preserve": ["event_id", "observed_time", "sequence", "symbol", "payload"],
        }),
        PolicyKind::ReceivedTimeLagCumulativeLookahead {
            roles_affected,
            max_lag_inclusive,
            threshold_pct_bps,
            pct_bps,
        } => json!({
            "name": policy.name.as_str(),
            "category": policy.category.as_str(),
            "kind": "received_time_lag_cumulative_lookahead",
            "time_axis": "event_lag_fixture_native_integer",
            "shift_units": "percent_of_each_event_lag",
            "calendar_aware": false,
            "bounded_by_observed_time": true,
            "roles_affected": roles_affected
                .iter()
                .map(|role| role.as_str())
                .collect::<Vec<_>>(),
            "max_lag_inclusive": max_lag_inclusive,
            "threshold_percent_bps": threshold_pct_bps,
            "threshold_percent": f64::from(*threshold_pct_bps) / 100.0,
            "lag_fraction_bps": pct_bps,
            "lag_fraction_percent": f64::from(*pct_bps) / 100.0,
            "preserve": ["event_id", "observed_time", "sequence", "symbol", "payload"],
        }),
        PolicyKind::ReplayOrderOverride { order } => json!({
            "name": policy.name.as_str(),
            "category": policy.category.as_str(),
            "kind": "replay_order_override",
            "order": replay_order_name(*order),
        }),
    }
}

fn replay_order_name(order: ReplayOrder) -> &'static str {
    match order {
        ReplayOrder::ReceivedTime => "received_time",
        ReplayOrder::ObservedTimeLeaky => "observed_time",
    }
}

fn print_sensitivity_stdout(
    args: &SensitivityArgs,
    sweep: &SensitivitySweep,
    artifacts: SensitivityArtifactPaths<'_>,
) {
    println!("asof-causality sensitivity");
    println!("  fixture   {}", args.events_path);
    println!("  signal    {}", args.signal.as_str());
    println!("  scenario  {}", args.scenario.as_str());
    println!("  baseline  {}", sweep.baseline.policy.name);
    println!(
        "  transcript_hash  {}",
        sweep.baseline.output.predictions.transcript_digest()
    );
    println!();
    println!("POLICIES");
    for result in &sweep.results {
        println!(
            "  [{}] {}  changed={}/{} flip_rate={:.4} new_inputs={}",
            result.run.policy.category.as_str(),
            result.run.policy.name,
            result.summary.predictions_with_signal_change,
            result.summary.predictions,
            result.summary.flip_rate,
            result.summary.new_input_uses
        );
    }
    println!();
    println!("ARTIFACTS  {}", args.out_dir.display());
    println!("  summary    {}", artifacts.summary.display());
    if let Some(sensitivity_curve_svg) = artifacts.sensitivity_curve_svg {
        println!("  curve svg  {}", sensitivity_curve_svg.display());
    }
    println!("  flip svg   {}", artifacts.flip_rate_svg.display());
    println!("  input svg  {}", artifacts.input_change_svg.display());
    if let Some(late_arrival_impact_svg) = artifacts.late_arrival_impact_svg {
        println!("  late svg   {}", late_arrival_impact_svg.display());
    }
    if let Some(details_path) = artifacts.details {
        println!("  details    {}", details_path.display());
    }
    println!("  manifest   {}", artifacts.manifest.display());
}

fn load_stored_predictions(
    path: &Path,
) -> Result<BTreeMap<AuditKey, StoredPrediction>, Box<dyn Error>> {
    let input = fs::read_to_string(path)?;
    parse_stored_predictions_jsonl(&input)
}

fn parse_stored_predictions_jsonl(
    input: &str,
) -> Result<BTreeMap<AuditKey, StoredPrediction>, Box<dyn Error>> {
    let mut stored = BTreeMap::new();

    for (index, record) in parse_jsonl_records::<StoredPredictionJson>(input, "stored prediction")?
    {
        let key = AuditKey {
            symbol: record.symbol,
            prediction_replay_key: record.prediction_replay_key,
        };

        if stored
            .insert(
                key,
                StoredPrediction {
                    signal_value: record.signal_value,
                    feature_recipe_hash: record.feature_recipe_hash,
                },
            )
            .is_some()
        {
            return Err(format!("duplicate stored prediction key on line {}", index + 1).into());
        }
    }

    Ok(stored)
}

fn load_outcome_attributions(
    path: &Path,
) -> Result<BTreeMap<AuditKey, OutcomeAttribution>, Box<dyn Error>> {
    let input = fs::read_to_string(path)?;
    if first_data_line(&input)
        .map(|line| line.starts_with('{'))
        .unwrap_or(false)
    {
        parse_outcome_jsonl(&input)
    } else {
        parse_outcome_pipe(&input)
    }
}

fn first_data_line(input: &str) -> Option<&str> {
    input.lines().find_map(data_line)
}

fn parse_outcome_jsonl(
    input: &str,
) -> Result<BTreeMap<AuditKey, OutcomeAttribution>, Box<dyn Error>> {
    let mut outcomes = BTreeMap::new();

    for (index, record) in parse_jsonl_records::<OutcomeJson>(input, "outcome")? {
        insert_outcome(
            &mut outcomes,
            index,
            AuditKey {
                symbol: record.symbol,
                prediction_replay_key: record.prediction_replay_key,
            },
            OutcomeAttribution {
                return_bps: record.return_bps,
            },
        )?;
    }

    Ok(outcomes)
}

fn parse_outcome_pipe(
    input: &str,
) -> Result<BTreeMap<AuditKey, OutcomeAttribution>, Box<dyn Error>> {
    let events = parse_pipe_events(input)?;
    let mut outcomes = BTreeMap::new();

    for (index, event) in events.iter().enumerate() {
        if event.role != EventRole::Outcome {
            continue;
        }

        let Some(prediction_replay_key) = payload_field(&event.payload, "prediction_replay_key")
        else {
            continue;
        };
        let Some(return_bps) = payload_field(&event.payload, "return_bps") else {
            continue;
        };
        let return_bps = parse_number_literal(&return_bps)
            .map_err(|error| format!("outcome line {} {error}", index + 1))?;
        insert_outcome(
            &mut outcomes,
            index,
            AuditKey {
                symbol: event.symbol.clone(),
                prediction_replay_key,
            },
            OutcomeAttribution { return_bps },
        )?;
    }

    Ok(outcomes)
}

fn insert_outcome(
    outcomes: &mut BTreeMap<AuditKey, OutcomeAttribution>,
    index: usize,
    key: AuditKey,
    outcome: OutcomeAttribution,
) -> Result<(), Box<dyn Error>> {
    if outcomes.insert(key, outcome).is_some() {
        return Err(format!("duplicate outcome key on line {}", index + 1).into());
    }

    Ok(())
}

fn payload_field(payload: &str, field: &str) -> Option<String> {
    payload.split(',').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key.trim() == field).then(|| value.trim().to_string())
    })
}

fn parse_jsonl_records<T>(input: &str, record_name: &str) -> Result<Vec<(usize, T)>, Box<dyn Error>>
where
    T: DeserializeOwned,
{
    let mut records = Vec::new();

    for (index, line) in input.lines().enumerate() {
        let Some(line) = data_line(line) else {
            continue;
        };
        let record = serde_json::from_str(line).map_err(|error| {
            format!("invalid {record_name} JSON on line {}: {error}", index + 1)
        })?;
        records.push((index, record));
    }

    Ok(records)
}

fn data_line(line: &str) -> Option<&str> {
    let line = line.trim().trim_start_matches('\u{feff}');
    (!line.is_empty() && !line.starts_with('#')).then_some(line)
}

fn parse_number_literal(value: &str) -> Result<Number, Box<dyn Error>> {
    let value = value.trim();
    if value.is_empty() {
        return Err("number literal is empty".into());
    }
    if let Ok(integer) = value.parse::<i64>() {
        return Ok(Number::from(integer));
    }
    let float = value.parse::<f64>()?;
    Number::from_f64(float).ok_or_else(|| "number literal must be finite".into())
}

struct EventLabels<'a> {
    event_labels: BTreeMap<EventKey, String>,
    symbol_labels: BTreeMap<SymbolId, String>,
    events_by_key: BTreeMap<EventKey, &'a Event>,
}

impl<'a> EventLabels<'a> {
    fn new(events: &'a [Event]) -> Self {
        Self {
            event_labels: events
                .iter()
                .map(|event| (event.event_key, event.event_id.clone()))
                .collect(),
            symbol_labels: events
                .iter()
                .map(|event| (event.symbol_key, event.symbol.clone()))
                .collect(),
            events_by_key: events
                .iter()
                .map(|event| (event.event_key, event))
                .collect(),
        }
    }
}

fn print_check_stdout(
    path: &str,
    signal: SignalChoice,
    options: CheckOptions,
    events: &[Event],
    report: &CheckReport,
    replay: Option<&ReplayOutput>,
) {
    println!("asof-causality check");
    println!("  fixture    {path}");
    println!("  events     {}", events.len());
    println!("  signal     {}", signal.as_str());
    println!("  cutoffs    {}", cutoff_summary(events, options));
    println!();
    print_check_section(report, true);
    println!();
    println!("PROVENANCE");
    match replay {
        Some(output) => {
            println!(
                "  transcript_hash      {:016x}",
                output.predictions.transcript_hash()
            );
            println!(
                "  predictions_emitted  {}",
                output.predictions.records().len()
            );
            println!("  outcomes_separated   {}", output.outcomes_seen);
        }
        None => {
            println!("  transcript_hash      unavailable");
            println!("  predictions_emitted  unavailable");
            println!("  outcomes_separated   unavailable");
        }
    }
}

fn print_negative_control_stdout(
    path: &str,
    signal: SignalChoice,
    events: &[Event],
    received_time: &ReplayOutput,
    observed_time: &ReplayOutput,
    labels: &EventLabels<'_>,
) {
    println!("asof-causality negative-control");
    println!("  fixture  {path}");
    println!("  events   {}", events.len());
    println!("  signal   {}", signal.as_str());
    println!();
    print_engine_summary(
        "ENGINE A: received-time replay (correct)",
        "(received_time, sequence, event_id)",
        received_time,
    );
    println!();
    print_engine_summary(
        "ENGINE B: observed-time replay (deliberately broken baseline)",
        "(observed_time, sequence, event_id)",
        observed_time,
    );
    println!();
    print_leaked_predictions(observed_time, labels);
    println!();
    print_negative_control_diagnostic(received_time, observed_time, labels);
}

fn print_engine_summary(name: &str, ordering: &str, output: &ReplayOutput) {
    let impossible = output.predictions.impossible_predictions();
    let verdict = if impossible.is_empty() {
        "PASS"
    } else {
        "FAIL"
    };

    println!("{name}");
    println!("  ordering             {ordering}");
    println!(
        "  transcript_hash      {:016x}",
        output.predictions.transcript_hash()
    );
    println!("  impossible           {}", impossible.len());
    println!("  VERDICT              {verdict}");
}

fn print_leaked_predictions(output: &ReplayOutput, labels: &EventLabels<'_>) {
    println!("LEAKED PREDICTIONS (engine B)");
    let impossible = output.predictions.impossible_predictions();

    if impossible.is_empty() {
        println!("  none");
        return;
    }

    for record in impossible {
        let prediction_event = labels.events_by_key.get(&record.prediction_event_key);
        let input_event = record
            .max_input_event_key
            .and_then(|event_key| labels.events_by_key.get(&event_key).copied());

        println!();
        match (prediction_event, input_event) {
            (Some(prediction), Some(input)) => {
                println!(
                    "  {} at {}",
                    prediction.event_id,
                    format_replay_key_for_event(prediction)
                );
                println!("    signal_value     {}", record.signal_value);
                println!(
                    "    leaked_input     {:<18} at {}",
                    input.event_id,
                    format_replay_key_for_event(input)
                );
                println!("    violation        {}", leak_violation(prediction, input));
                println!(
                    "    interpretation   {}",
                    leak_interpretation(prediction, input)
                );
            }
            _ => {
                println!(
                    "  {}",
                    record.canonical_line(&labels.event_labels, &labels.symbol_labels)
                );
                println!("    violation        input replay key > prediction replay key");
            }
        }
    }
}

fn print_negative_control_diagnostic(
    received_time: &ReplayOutput,
    observed_time: &ReplayOutput,
    labels: &EventLabels<'_>,
) {
    let correct_impossible = received_time.predictions.impossible_predictions();
    let leaky_impossible = observed_time.predictions.impossible_predictions();
    let leak_classes = leaky_impossible
        .iter()
        .filter_map(|record| {
            let prediction = labels.events_by_key.get(&record.prediction_event_key)?;
            let input = record
                .max_input_event_key
                .and_then(|event_key| labels.events_by_key.get(&event_key).copied())?;
            Some(leak_class(prediction, input))
        })
        .collect::<BTreeSet<_>>()
        .len();

    println!("DIAGNOSTIC");
    if leaky_impossible.is_empty() {
        println!("  the broken engine emitted 0 impossible predictions on this fixture");
    } else {
        println!(
            "  the broken engine emitted {} impossible predictions across {} distinct leak classes",
            leaky_impossible.len(),
            leak_classes
        );
    }
    println!("  the correct engine emitted {}", correct_impossible.len());
    println!("  the audit invariant catches the failure mode the engine is designed to prevent");
}

fn format_replay_key_for_event(event: &Event) -> String {
    format!(
        "({}, {}, {})",
        event.received_time, event.sequence, event.event_id
    )
}

fn leak_class(prediction: &Event, input: &Event) -> &'static str {
    if input.received_time == prediction.received_time {
        "same-timestamp sequence"
    } else if input.role == EventRole::FeatureCorrection {
        "late correction"
    } else {
        "late arrival"
    }
}

fn leak_violation(prediction: &Event, input: &Event) -> String {
    if input.received_time == prediction.received_time {
        "input sequence > prediction sequence at same received_time".to_string()
    } else {
        format!(
            "input replay key > prediction replay key by delta={}",
            input.received_time.saturating_sub(prediction.received_time)
        )
    }
}

fn leak_interpretation(prediction: &Event, input: &Event) -> String {
    if input.received_time == prediction.received_time {
        format!(
            "prediction at t={} used same-timestamp event that sorts after it",
            prediction.received_time
        )
    } else if input.role == EventRole::FeatureCorrection {
        format!(
            "prediction at t={} used correction received at t={}",
            prediction.received_time, input.received_time
        )
    } else {
        format!(
            "prediction at t={} used event that arrived at t={}",
            prediction.received_time, input.received_time
        )
    }
}

fn print_check_section(report: &CheckReport, include_details: bool) {
    let passed = passed_check_count(report);
    let total = report.results.len();
    println!(
        "{:<58} {}/{} {}",
        "ADVERSARIAL CHECKS",
        passed,
        total,
        overall_status(report)
    );
    for result in &report.results {
        let status = if result.passed { "PASS" } else { "FAIL" };
        if include_details {
            println!(
                "  [{status}]  {:<32} {}",
                result.name,
                shortened_check_detail(result.name, &result.detail)
            );
        } else {
            println!("  [{status}]  {}", result.name);
        }
    }
}

fn passed_check_count(report: &CheckReport) -> usize {
    report.results.iter().filter(|result| result.passed).count()
}

fn overall_status(report: &CheckReport) -> &'static str {
    if report.passed() {
        "PASS"
    } else {
        "FAIL"
    }
}

fn shortened_check_detail<'a>(name: &str, detail: &'a str) -> &'a str {
    if name == "deterministic_replay" && detail.starts_with("shuffled input produced transcript") {
        "shuffled input produced same transcript hash"
    } else {
        detail
    }
}

fn cutoff_summary(events: &[Event], options: CheckOptions) -> String {
    let total = prediction_cutoff_count(events);
    match options.max_cutoffs {
        None => format!("exhaustive ({total})"),
        Some(max_cutoffs) => {
            let used = selected_cutoff_count(total, max_cutoffs);
            if used == total {
                format!("all {total} (max {max_cutoffs})")
            } else {
                format!("sampled {used} of {total} (max {max_cutoffs})")
            }
        }
    }
}

fn prediction_cutoff_count(events: &[Event]) -> usize {
    let mut cutoffs: Vec<u64> = events
        .iter()
        .filter(|event| event.role == EventRole::Prediction)
        .map(|event| event.received_time)
        .collect();
    cutoffs.sort_unstable();
    cutoffs.dedup();
    cutoffs.len()
}

fn selected_cutoff_count(total: usize, max_cutoffs: usize) -> usize {
    if total == 0 || max_cutoffs == 0 {
        return 0;
    }
    if total <= max_cutoffs {
        return total;
    }
    if max_cutoffs == 1 {
        return 1;
    }

    let last = total - 1;
    (0..max_cutoffs)
        .map(|index| index * last / (max_cutoffs - 1))
        .collect::<BTreeSet<_>>()
        .len()
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

struct RunSuiteStdout<'a> {
    out_dir: &'a Path,
    manifest_path: &'a Path,
    stream: &'a GeneratedStream,
    signal: SignalChoice,
    replay: &'a ReplayOutput,
    replay_elapsed: Duration,
    report: &'a CheckReport,
    manifest: &'a RunManifest,
}

fn print_run_suite_stdout(inputs: RunSuiteStdout<'_>) {
    println!("asof-causality run-suite");
    println!("  scenario   {}", inputs.stream.stats.scenario.as_str());
    println!("  seed       {}", inputs.stream.stats.seed);
    println!("  signal     {}", inputs.signal.as_str());
    println!();

    println!("PHASE 1  GENERATE");
    println!("  events            {}", inputs.stream.stats.data_events);
    println!(
        "  rows              {}  (includes corrections, predictions, outcomes)",
        inputs.stream.stats.rows
    );
    println!("  symbols           {}", inputs.stream.stats.symbols);
    println!(
        "  late_updates      {}  ({})",
        inputs.stream.stats.late_updates,
        percent(
            inputs.stream.stats.late_updates,
            inputs.stream.stats.data_events
        )
    );
    println!(
        "  corrections       {}  ({})",
        inputs.stream.stats.feature_corrections,
        percent(
            inputs.stream.stats.feature_corrections,
            inputs.stream.stats.data_events
        )
    );
    println!("  predictions       {}", inputs.stream.stats.predictions);
    println!(
        "  physical_order    {}",
        if inputs.stream.stats.shuffled {
            "shuffled  (replay order must reconstruct from replay key)"
        } else {
            "ordered"
        }
    );
    println!();

    println!("PHASE 2  REPLAY  ordered by (received_time, sequence, event_id)");
    println!("  events_replayed   {}", inputs.replay.replayed_events);
    println!(
        "  predictions       {}",
        inputs.replay.predictions.records().len()
    );
    println!("  outcomes_seen     {}", inputs.replay.outcomes_seen);
    println!(
        "  throughput        {} events/sec  (symbol-id state representation)",
        format_rate(events_per_second(
            inputs.replay.replayed_events,
            inputs.replay_elapsed
        ))
    );
    println!();

    print_check_section(inputs.report, false);
    let used_cutoffs = selected_cutoff_count(prediction_cutoff_count(&inputs.stream.events), 32);
    let total_cutoffs = prediction_cutoff_count(&inputs.stream.events);
    println!(
        "  cutoffs_sampled   {} of {}  (deterministic sampling for large fixtures)",
        used_cutoffs, total_cutoffs
    );
    println!();

    print!(
        "{}",
        format_provenance_stdout(inputs.manifest_path, inputs.manifest)
    );

    println!("ARTIFACTS  {}", inputs.out_dir.display());
    println!("  events.pipe        {} rows", inputs.stream.stats.rows);
    println!(
        "  predictions.pipe   {} records",
        inputs.replay.predictions.records().len()
    );
    println!(
        "  checks.txt         {} results",
        inputs.report.results.len()
    );
    println!("  summary.md");
    println!("  manifest.json      hash-linked run identity");
    println!();

    println!(
        "RESULT     {}  ({}/{} checks)",
        overall_status(inputs.report),
        passed_check_count(inputs.report),
        inputs.report.results.len()
    );
}

fn format_provenance_stdout(manifest_path: &Path, manifest: &RunManifest) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "PROVENANCE  written to {}", manifest_path.display());
    let _ = writeln!(text, "  hash_algorithm         {}", manifest.hash_algorithm);
    let _ = writeln!(
        text,
        "  data_fixture_hash      {}",
        manifest.data_fixture_hash
    );
    let _ = writeln!(
        text,
        "  prediction_output_hash {}",
        manifest.prediction_output_hash
    );
    let _ = writeln!(
        text,
        "  checks_output_hash     {}",
        manifest.checks_output_hash
    );
    let _ = writeln!(
        text,
        "  transcript_hash        {}",
        manifest.transcript_hash
    );
    let _ = writeln!(text);
    let _ = writeln!(text, "RUN CONTEXT");
    let _ = writeln!(
        text,
        "  source_commit          {}",
        manifest
            .source_commit
            .as_deref()
            .map(short_hash)
            .unwrap_or("unavailable")
    );
    let _ = writeln!(
        text,
        "  workspace_dirty        {}",
        manifest
            .workspace_dirty
            .map(|dirty| dirty.to_string())
            .unwrap_or_else(|| "unavailable".to_string())
    );
    let _ = writeln!(
        text,
        "  rust_toolchain         {}",
        manifest.rust_toolchain.as_deref().unwrap_or("unavailable")
    );
    let _ = writeln!(
        text,
        "  run_started_utc        {}",
        manifest.run_started_utc
    );
    let _ = writeln!(text);
    text
}

fn events_per_second(events: usize, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64();
    if seconds > 0.0 {
        events as f64 / seconds
    } else {
        0.0
    }
}

fn format_rate(rate: f64) -> String {
    if rate >= 1_000_000.0 {
        format!("{:.1}M", rate / 1_000_000.0)
    } else if rate >= 1_000.0 {
        format!("{:.1}K", rate / 1_000.0)
    } else {
        format!("{rate:.0}")
    }
}

fn percent(count: usize, total: usize) -> String {
    if total == 0 {
        "0.0%".to_string()
    } else {
        format!("{:.1}%", count as f64 * 100.0 / total as f64)
    }
}

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckCounts {
    passed: usize,
    failed: usize,
    total: usize,
}

impl CheckCounts {
    fn from_report(report: &CheckReport) -> Self {
        let passed = passed_check_count(report);
        let total = report.results.len();
        Self {
            passed,
            failed: total.saturating_sub(passed),
            total,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RunManifest {
    schema_version: u8,
    tool: &'static str,
    hash_algorithm: &'static str,
    run_started_utc: String,
    invocation: String,
    invocation_args: Vec<String>,
    source_commit: Option<String>,
    workspace_dirty: Option<bool>,
    rust_toolchain: Option<String>,
    scenario: String,
    signal: String,
    seed: u64,
    events_requested: usize,
    rows: usize,
    symbols: usize,
    late_rate: f64,
    feature_correction_rate: f64,
    outcome_rate: f64,
    data_fixture_hash: String,
    prediction_output_hash: String,
    checks_output_hash: String,
    transcript_hash: String,
    checks_passed: bool,
    checks: CheckCounts,
}

impl RunManifest {
    fn new(inputs: RunManifestInputs<'_>) -> Self {
        Self::new_with_context(
            inputs,
            SystemTime::now(),
            env::args().collect(),
            current_git_commit(),
            current_workspace_dirty(),
            current_rustc_version(),
        )
    }

    fn new_with_context(
        inputs: RunManifestInputs<'_>,
        run_started: SystemTime,
        invocation_args: Vec<String>,
        source_commit: Option<String>,
        workspace_dirty: Option<bool>,
        rust_toolchain: Option<String>,
    ) -> Self {
        Self {
            schema_version: 3,
            tool: "asof-causality",
            hash_algorithm: "blake3",
            run_started_utc: system_time_to_utc_iso8601(run_started),
            invocation: shell_join(&invocation_args),
            invocation_args,
            source_commit,
            workspace_dirty,
            rust_toolchain,
            scenario: inputs.stream.stats.scenario.as_str().to_string(),
            signal: inputs.signal.as_str().to_string(),
            seed: inputs.stream.stats.seed,
            events_requested: inputs.config.events,
            rows: inputs.stream.stats.rows,
            symbols: inputs.stream.stats.symbols,
            late_rate: inputs.config.late_rate,
            feature_correction_rate: inputs.config.feature_correction_rate,
            outcome_rate: inputs.config.outcome_rate,
            data_fixture_hash: asof_causality_core::blake3_hex(inputs.events_output.as_bytes()),
            prediction_output_hash: asof_causality_core::blake3_hex(
                inputs.predictions_output.as_bytes(),
            ),
            checks_output_hash: asof_causality_core::blake3_hex(inputs.checks_output.as_bytes()),
            transcript_hash: inputs.replay.predictions.transcript_digest(),
            checks_passed: inputs.report.passed(),
            checks: CheckCounts::from_report(inputs.report),
        }
    }
}

fn format_run_manifest(manifest: &RunManifest) -> String {
    let mut text = String::new();

    let _ = writeln!(text, "{{");
    let _ = writeln!(text, "  \"schema_version\": {},", manifest.schema_version);
    let _ = writeln!(text, "  \"tool\": \"{}\",", manifest.tool);
    let _ = writeln!(
        text,
        "  \"hash_algorithm\": \"{}\",",
        manifest.hash_algorithm
    );
    let _ = writeln!(
        text,
        "  \"run_started_utc\": \"{}\",",
        manifest.run_started_utc
    );
    let _ = writeln!(
        text,
        "  \"invocation\": \"{}\",",
        json_escape(&manifest.invocation)
    );
    let _ = writeln!(
        text,
        "  \"invocation_args\": {},",
        json_string_array(&manifest.invocation_args)
    );
    let _ = writeln!(
        text,
        "  \"source_commit\": {},",
        json_optional(&manifest.source_commit)
    );
    let _ = writeln!(
        text,
        "  \"workspace_dirty\": {},",
        json_optional_bool(manifest.workspace_dirty)
    );
    let _ = writeln!(
        text,
        "  \"rust_toolchain\": {},",
        json_optional(&manifest.rust_toolchain)
    );
    let _ = writeln!(text, "  \"scenario\": \"{}\",", manifest.scenario);
    let _ = writeln!(text, "  \"signal\": \"{}\",", manifest.signal);
    let _ = writeln!(text, "  \"seed\": {},", manifest.seed);
    let _ = writeln!(
        text,
        "  \"events_requested\": {},",
        manifest.events_requested
    );
    let _ = writeln!(text, "  \"rows\": {},", manifest.rows);
    let _ = writeln!(text, "  \"symbols\": {},", manifest.symbols);
    let _ = writeln!(text, "  \"late_rate\": {},", manifest.late_rate);
    let _ = writeln!(
        text,
        "  \"feature_correction_rate\": {},",
        manifest.feature_correction_rate
    );
    let _ = writeln!(text, "  \"outcome_rate\": {},", manifest.outcome_rate);
    let _ = writeln!(
        text,
        "  \"data_fixture_hash\": \"{}\",",
        manifest.data_fixture_hash
    );
    let _ = writeln!(
        text,
        "  \"prediction_output_hash\": \"{}\",",
        manifest.prediction_output_hash
    );
    let _ = writeln!(
        text,
        "  \"checks_output_hash\": \"{}\",",
        manifest.checks_output_hash
    );
    let _ = writeln!(
        text,
        "  \"transcript_hash\": \"{}\",",
        manifest.transcript_hash
    );
    let _ = writeln!(text, "  \"checks_passed\": {},", manifest.checks_passed);
    let _ = writeln!(
        text,
        "  \"checks\": {{ \"passed\": {}, \"failed\": {}, \"total\": {} }}",
        manifest.checks.passed, manifest.checks.failed, manifest.checks.total
    );
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

fn current_workspace_dirty() -> Option<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(!output.stdout.is_empty())
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

fn json_optional_bool(value: Option<bool>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_string())
}

fn json_string_array(values: &[String]) -> String {
    let mut text = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            text.push_str(", ");
        }
        let _ = write!(text, "\"{}\"", json_escape(value));
    }
    text.push(']');
    text
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|arg| shell_escape(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    if value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'-' | b'.' | b'/' | b':' | b'=' | b'@')
    }) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn system_time_to_utc_iso8601(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    unix_seconds_to_utc_iso8601(seconds)
}

fn unix_seconds_to_utc_iso8601(seconds: u64) -> String {
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };

    (year, month as u32, day as u32)
}

fn print_help() {
    println!("asof-causality");
    println!();
    println!("usage:");
    println!("  asof-causality replay [path] [--signal name]");
    println!("  asof-causality check [path] [--signal name] [--max-cutoffs N|--exhaustive]");
    println!("  asof-causality audit [events] [stored_predictions.jsonl] [legacy_outcomes] [--signal name] [--out path] [--outcomes path] [--allow-missing-recipe-hash]");
    println!("      prefer --outcomes path when attaching outcomes without stored predictions");
    println!("  asof-causality negative-control [path] [--signal name]");
    println!("  asof-causality generate [--scenario late-heavy] [--events N] [--symbols N] [--late-rate R] [--feature-correction-rate R] [--outcome-rate R] [--seed N] [--out path]");
    println!("  asof-causality run-suite [--scenario late-heavy] [--signal name] [--events N] [--symbols N] [--late-rate R] [--feature-correction-rate R] [--outcome-rate R] [--seed N] [--out dir]");
    println!("  asof-causality sensitivity [path] [--signal name] [--scenario lookahead|late-arrivals] [--lookahead-range 0..100] [--late-arrival-buckets auto] [--steps N] [--details] --out dir");
    println!("  asof-causality bench [--events N] [--symbols N]");
    println!();
    println!("signals:");
    println!("  last-feature-sentiment (default)");
    println!("  windowed-feature-sentiment");
    println!("  windowed-zscore");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn late_arrival_events() -> Vec<Event> {
        parse_pipe_events(include_str!("../../../examples/late-arrival.pipe")).unwrap()
    }

    fn negative_control_events() -> Vec<Event> {
        parse_pipe_events(include_str!(
            "../../../examples/lookahead-negative-control.pipe"
        ))
        .unwrap()
    }

    fn sample_manifest() -> RunManifest {
        let config = GenerateConfig {
            events: 64,
            symbols: 4,
            seed: 99,
            ..GenerateConfig::for_scenario(Scenario::LateHeavy)
        };
        let stream = generate_events(&config);
        let replay = replay_with_signal(
            SignalChoice::LastFeatureSentiment,
            &stream.events,
            ReplayOptions::default(),
            ReplayOrder::ReceivedTime,
        )
        .unwrap();
        let report = run_checks_with_signal(
            SignalChoice::LastFeatureSentiment,
            &stream.events,
            CheckOptions::sampled(8),
        );
        let events_output = stream.to_pipe_string();
        let predictions_output = format_prediction_output(&replay);
        let checks_output = format_check_report(&report);

        RunManifest::new_with_context(
            RunManifestInputs {
                config: &config,
                stream: &stream,
                signal: SignalChoice::LastFeatureSentiment,
                replay: &replay,
                report: &report,
                events_output: &events_output,
                predictions_output: &predictions_output,
                checks_output: &checks_output,
            },
            UNIX_EPOCH + Duration::from_secs(1_704_067_200),
            args(&[
                "asof-causality",
                "run-suite",
                "--out",
                "/tmp/path with space",
            ]),
            Some("abcdef1234567890".to_string()),
            Some(false),
            Some("rustc test".to_string()),
        )
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
    fn parses_windowed_zscore_signal() {
        let (_, signal) = parse_path_signal_args(
            &args(&["--signal", "windowed-zscore"]),
            "default.pipe",
            "replay",
        )
        .unwrap();

        assert_eq!(signal, SignalChoice::WindowedZScore);
    }

    #[test]
    fn parses_signal_for_run_suite() {
        let (_, _, signal) =
            parse_generate_args(&args(&["--signal", "windowed-feature-sentiment"]), true).unwrap();

        assert_eq!(signal, SignalChoice::WindowedFeatureSentiment);
    }

    #[test]
    fn parses_audit_args() {
        let parsed = parse_audit_args(&args(&[
            "examples/lookahead-negative-control.pipe",
            "runs/stored.jsonl",
            "runs/outcomes.pipe",
            "--signal",
            "windowed-feature-sentiment",
            "--out",
            "runs/audit.jsonl",
            "--allow-missing-recipe-hash",
        ]))
        .unwrap();

        assert_eq!(
            parsed.events_path,
            "examples/lookahead-negative-control.pipe"
        );
        assert_eq!(
            parsed.stored_predictions_path,
            Some(PathBuf::from("runs/stored.jsonl"))
        );
        assert_eq!(
            parsed.outcomes_path,
            Some(PathBuf::from("runs/outcomes.pipe"))
        );
        assert_eq!(parsed.signal, SignalChoice::WindowedFeatureSentiment);
        assert_eq!(parsed.out, Some(PathBuf::from("runs/audit.jsonl")));
        assert!(parsed.allow_missing_recipe_hash);
    }

    #[test]
    fn parses_audit_outcomes_flag() {
        let parsed = parse_audit_args(&args(&[
            "examples/alfred-dgs10-sp500.pipe",
            "--outcomes",
            "examples/alfred-dgs10-sp500.pipe",
        ]))
        .unwrap();

        assert_eq!(parsed.events_path, "examples/alfred-dgs10-sp500.pipe");
        assert_eq!(parsed.stored_predictions_path, None);
        assert_eq!(
            parsed.outcomes_path,
            Some(PathBuf::from("examples/alfred-dgs10-sp500.pipe"))
        );
    }

    #[test]
    fn parses_sensitivity_args() {
        let parsed = parse_sensitivity_args(&args(&[
            "examples/alfred-dgs10-sp500.pipe",
            "--signal",
            "windowed-zscore",
            "--shift-features",
            "-10000",
            "--observed-time-leaky",
            "--details",
            "--out",
            "runs/sensitivity",
        ]))
        .unwrap();

        assert_eq!(parsed.events_path, "examples/alfred-dgs10-sp500.pipe");
        assert_eq!(parsed.signal, SignalChoice::WindowedZScore);
        assert_eq!(parsed.out_dir, PathBuf::from("runs/sensitivity"));
        assert!(parsed.details);
        assert_eq!(parsed.policies.len(), 2);
        assert_eq!(parsed.policies[0].name, "shift_features_minus_10000");
        assert_eq!(parsed.policies[1].name, "observed_time_leaky");
    }

    #[test]
    fn parses_normalized_lookahead_range_args() {
        let parsed = parse_sensitivity_args(&args(&[
            "examples/late-arrival.pipe",
            "--lookahead-range",
            "0..100",
            "--steps",
            "4",
            "--out",
            "runs/sensitivity",
        ]))
        .unwrap();

        assert_eq!(parsed.policies.len(), 4);
        assert_eq!(parsed.scenario, SensitivityScenario::Lookahead);
        assert_eq!(parsed.policies[0].name, "lookahead_25pct");
        assert_eq!(parsed.policies[3].name, "lookahead_100pct");
        assert!(matches!(
            parsed.policies[0].kind,
            PolicyKind::ReceivedTimeLagFraction { pct_bps: 2500, .. }
        ));
    }

    #[test]
    fn parses_late_arrival_scenario_args() {
        let parsed = parse_sensitivity_args(&args(&[
            "examples/late-arrival.pipe",
            "--scenario",
            "late-arrivals",
            "--out",
            "runs/sensitivity",
        ]))
        .unwrap();

        assert_eq!(parsed.scenario, SensitivityScenario::LateArrivals);
        assert_eq!(
            parsed.late_arrival_buckets,
            Some(LateArrivalBucketSpec::Auto)
        );
        assert!(parsed.policies.is_empty());
    }

    #[test]
    fn auto_late_arrival_policies_cover_cumulative_lag_thresholds() {
        let policies = cumulative_late_arrival_policies(&negative_control_events(), 4).unwrap();

        assert!(!policies.is_empty());
        assert!(policies
            .iter()
            .any(|policy| policy.name.starts_with("late_arrivals_cumulative_")));
        assert!(matches!(
            policies[0].kind,
            PolicyKind::ReceivedTimeLagCumulativeLookahead {
                pct_bps: 10_000,
                ..
            }
        ));
    }

    #[test]
    fn sensitivity_rejects_typed_duration_shifts() {
        let error = parse_sensitivity_args(&args(&[
            "--shift-features",
            "-1d",
            "--out",
            "runs/sensitivity",
        ]))
        .unwrap_err()
        .to_string();

        assert!(error.contains("typed duration shifts"));
    }

    #[test]
    fn sensitivity_requires_out_and_policy() {
        let missing_out = parse_sensitivity_args(&args(&["--shift-features", "-10"]))
            .unwrap_err()
            .to_string();
        assert!(missing_out.contains("--out"));

        let missing_policy = parse_sensitivity_args(&args(&["--out", "runs/sensitivity"]))
            .unwrap_err()
            .to_string();
        assert!(missing_policy.contains("comparison policy"));

        let steps_without_sweep =
            parse_sensitivity_args(&args(&["--steps", "4", "--out", "runs/sensitivity"]))
                .unwrap_err()
                .to_string();
        assert!(steps_without_sweep.contains("--lookahead-range"));
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

    #[test]
    fn manifest_v3_contains_required_fields() {
        let manifest = sample_manifest();
        let json = format_run_manifest(&manifest);

        assert!(json.contains("\"schema_version\": 3"));
        assert!(json.contains("\"tool\": \"asof-causality\""));
        assert!(json.contains("\"hash_algorithm\": \"blake3\""));
        assert!(json.contains("\"run_started_utc\": \"2024-01-01T00:00:00Z\""));
        assert!(json
            .contains("\"invocation\": \"asof-causality run-suite --out '/tmp/path with space'\""));
        assert!(json.contains("\"invocation_args\": [\"asof-causality\", \"run-suite\", \"--out\", \"/tmp/path with space\"]"));
        assert!(json.contains("\"source_commit\": \"abcdef1234567890\""));
        assert!(json.contains("\"workspace_dirty\": false"));
        assert!(json.contains("\"rust_toolchain\": \"rustc test\""));
        assert!(json.contains("\"data_fixture_hash\":"));
        assert!(json.contains("\"prediction_output_hash\":"));
        assert!(json.contains("\"checks_output_hash\":"));
        assert!(json.contains("\"transcript_hash\":"));
        assert!(json.contains("\"checks_passed\": true"));
    }

    #[test]
    fn manifest_records_check_summary_object() {
        let manifest = sample_manifest();
        let json = format_run_manifest(&manifest);

        assert_eq!(manifest.checks.passed, 8);
        assert_eq!(manifest.checks.failed, 0);
        assert_eq!(manifest.checks.total, 8);
        assert!(json.contains("\"checks\": { \"passed\": 8, \"failed\": 0, \"total\": 8 }"));
    }

    #[test]
    fn invocation_args_serialize_as_json_array_with_escaping() {
        let values = args(&["asof-causality", "quote\"arg", "slash\\arg"]);

        assert_eq!(
            json_string_array(&values),
            "[\"asof-causality\", \"quote\\\"arg\", \"slash\\\\arg\"]"
        );
    }

    #[test]
    fn utc_formatter_formats_fixed_timestamps() {
        assert_eq!(unix_seconds_to_utc_iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            unix_seconds_to_utc_iso8601(1_704_067_200),
            "2024-01-01T00:00:00Z"
        );
    }

    #[test]
    fn stdout_provenance_labels_match_manifest_keys() {
        let manifest = sample_manifest();
        let output = format_provenance_stdout(Path::new("manifest.json"), &manifest);

        assert!(output.contains("  hash_algorithm"));
        assert!(output.contains("  data_fixture_hash"));
        assert!(output.contains("  prediction_output_hash"));
        assert!(output.contains("  checks_output_hash"));
        assert!(output.contains("  transcript_hash"));
        assert!(output.contains("RUN CONTEXT"));
        assert!(output.contains("  source_commit"));
        assert!(output.contains("  workspace_dirty"));
        assert!(output.contains("  rust_toolchain"));
        assert!(output.contains("  run_started_utc"));
        assert!(!output.contains("  fixture_hash"));
        assert!(!output.contains("  predictions_hash"));
        assert!(!output.contains("  checks_hash"));
        assert!(!output.contains("  signal_version_hash"));
        assert!(!output.contains("  code_commit_hash"));
    }

    #[test]
    fn audit_jsonl_contains_schema_fields() {
        let events = late_arrival_events();
        let replay = replay_with_signal(
            SignalChoice::LastFeatureSentiment,
            &events,
            ReplayOptions::default(),
            ReplayOrder::ReceivedTime,
        )
        .unwrap();
        let outcomes = BTreeMap::new();
        let jsonl = format_audit_jsonl(
            SignalChoice::LastFeatureSentiment,
            &replay,
            None,
            &outcomes,
            false,
        );

        assert!(jsonl.contains("\"schema_version\":2"));
        assert!(jsonl.contains("\"prediction_id\":\"p1\""));
        assert!(jsonl.contains("\"signal\":\"last-feature-sentiment\""));
        assert!(jsonl.contains("\"prediction_replay_key\":\"580:3:p1\""));
        assert!(jsonl.contains("\"input_event_ids\":[]"));
        assert!(jsonl.contains("\"max_input_replay_key\":null"));
        assert!(jsonl.contains("\"feature_recipe_hash\":\""));
        assert!(jsonl.contains("\"causally_valid\":true"));
        assert!(jsonl.contains("\"matched_stored_prediction\":null"));
        assert!(jsonl.contains("\"outcome\":null"));
    }

    #[test]
    fn audit_jsonl_records_multi_input_provenance() {
        let events = negative_control_events();
        let replay = replay_with_signal(
            SignalChoice::WindowedFeatureSentiment,
            &events,
            ReplayOptions::default(),
            ReplayOrder::ReceivedTime,
        )
        .unwrap();
        let outcomes = BTreeMap::new();
        let jsonl = format_audit_jsonl(
            SignalChoice::WindowedFeatureSentiment,
            &replay,
            None,
            &outcomes,
            false,
        );

        assert!(jsonl.contains("\"signal\":\"windowed-feature-sentiment\""));
        assert!(jsonl.contains("\"input_event_ids\":[\"n_seed_negative\",\"n_seed_positive\""));
    }

    #[test]
    fn audit_jsonl_can_mark_non_causal_records() {
        let events = negative_control_events();
        let replay = replay_with_signal(
            SignalChoice::WindowedFeatureSentiment,
            &events,
            ReplayOptions::default(),
            ReplayOrder::ObservedTimeLeaky,
        )
        .unwrap();
        let outcomes = BTreeMap::new();
        let jsonl = format_audit_jsonl(
            SignalChoice::WindowedFeatureSentiment,
            &replay,
            None,
            &outcomes,
            false,
        );

        assert!(jsonl.contains("\"causally_valid\":false"));
    }

    #[test]
    fn audit_jsonl_marks_matching_stored_predictions() {
        let events = late_arrival_events();
        let replay = replay_with_signal(
            SignalChoice::LastFeatureSentiment,
            &events,
            ReplayOptions::default(),
            ReplayOrder::ReceivedTime,
        )
        .unwrap();
        let outcomes = BTreeMap::new();
        let expected = format_audit_jsonl(
            SignalChoice::LastFeatureSentiment,
            &replay,
            None,
            &outcomes,
            false,
        );
        let stored = parse_stored_predictions_jsonl(&expected).unwrap();
        let audited = format_audit_jsonl(
            SignalChoice::LastFeatureSentiment,
            &replay,
            Some(&stored),
            &outcomes,
            false,
        );

        assert!(audited.contains("\"matched_stored_prediction\":true"));
        assert!(!audited.contains("\"matched_stored_prediction\":false"));
    }

    #[test]
    fn audit_jsonl_marks_missing_stored_predictions() {
        let events = late_arrival_events();
        let replay = replay_with_signal(
            SignalChoice::LastFeatureSentiment,
            &events,
            ReplayOptions::default(),
            ReplayOrder::ReceivedTime,
        )
        .unwrap();
        let stored = BTreeMap::new();
        let outcomes = BTreeMap::new();
        let jsonl = format_audit_jsonl(
            SignalChoice::LastFeatureSentiment,
            &replay,
            Some(&stored),
            &outcomes,
            false,
        );

        assert!(jsonl.contains("\"matched_stored_prediction\":false"));
    }

    #[test]
    fn audit_jsonl_marks_mismatched_recipe_hash() {
        let stored_jsonl = "{\"prediction_replay_key\":\"580:3:p1\",\"symbol\":\"AAPL\",\"signal_value\":0,\"feature_recipe_hash\":\"0000000000000000000000000000000000000000000000000000000000000000\"}\n";
        let stored = parse_stored_predictions_jsonl(stored_jsonl).unwrap();
        let events = late_arrival_events();
        let replay = replay_with_signal(
            SignalChoice::LastFeatureSentiment,
            &events,
            ReplayOptions::default(),
            ReplayOrder::ReceivedTime,
        )
        .unwrap();
        let outcomes = BTreeMap::new();
        let jsonl = format_audit_jsonl(
            SignalChoice::LastFeatureSentiment,
            &replay,
            Some(&stored),
            &outcomes,
            false,
        );

        assert!(jsonl.contains("\"matched_stored_prediction\":false"));
    }

    #[test]
    fn stored_predictions_require_recipe_hash_by_default() {
        let stored_record = StoredPrediction {
            signal_value: 0,
            feature_recipe_hash: None,
        };

        assert!(!stored_record_matches(&stored_record, 0, "abc", false));
        assert!(stored_record_matches(&stored_record, 0, "abc", true));
    }

    #[test]
    fn stored_prediction_jsonl_accepts_valid_json_variants() {
        let stored = parse_stored_predictions_jsonl(
            "{\"symbol\":\"AAPL\", \"signal_value\":0, \"prediction_replay_key\":\"580:3:p\\u0031\"}\n",
        )
        .unwrap();

        assert!(stored.contains_key(&AuditKey {
            symbol: "AAPL".to_string(),
            prediction_replay_key: "580:3:p1".to_string(),
        }));
    }

    #[test]
    fn audit_jsonl_attaches_explicit_pipe_outcome() {
        let outcomes = parse_outcome_pipe(
            "o1|640|640|8|outcome|AAPL|prediction_replay_key=590:4:p2,return_bps=12\n",
        )
        .unwrap();
        let events = late_arrival_events();
        let replay = replay_with_signal(
            SignalChoice::LastFeatureSentiment,
            &events,
            ReplayOptions::default(),
            ReplayOrder::ReceivedTime,
        )
        .unwrap();
        let jsonl = format_audit_jsonl(
            SignalChoice::LastFeatureSentiment,
            &replay,
            None,
            &outcomes,
            false,
        );

        assert!(jsonl.contains("\"prediction_replay_key\":\"590:4:p2\""));
        assert!(jsonl.contains("\"outcome\":{\"return_bps\":12}"));
    }

    #[test]
    fn audit_jsonl_attaches_jsonl_outcome() {
        let outcomes = parse_outcome_jsonl(
            "{\"prediction_replay_key\":\"590:4:p2\",\"symbol\":\"AAPL\",\"return_bps\":12}\n",
        )
        .unwrap();
        let events = late_arrival_events();
        let replay = replay_with_signal(
            SignalChoice::LastFeatureSentiment,
            &events,
            ReplayOptions::default(),
            ReplayOrder::ReceivedTime,
        )
        .unwrap();
        let jsonl = format_audit_jsonl(
            SignalChoice::LastFeatureSentiment,
            &replay,
            None,
            &outcomes,
            false,
        );

        assert!(jsonl.contains("\"outcome\":{\"return_bps\":12}"));
    }
}
