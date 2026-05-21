use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cli() -> Result<Command, Box<dyn std::error::Error>> {
    let mut command = Command::cargo_bin("asof-causality")?;
    command.current_dir(repo_root());
    Ok(command)
}

fn success_output(args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    let output = cli()?.args(args).output()?;
    assert!(
        output.status.success(),
        "command {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "command {:?} wrote stderr on success:\n{}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output)
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout should be valid UTF-8")
}

fn schema_validator(path: &Path) -> Result<jsonschema::Validator, Box<dyn std::error::Error>> {
    let schema: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    Ok(jsonschema::validator_for(&schema)?)
}

fn validate_json(schema_path: &Path, value: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let validator = schema_validator(schema_path)?;
    validator.validate(value).map_err(|error| {
        format!(
            "{} failed schema validation against {}",
            error,
            schema_path.display()
        )
        .into()
    })
}

#[test]
fn audit_cli_round_trips_stored_prediction_jsonl() -> TestResult {
    let events = repo_root().join("examples/late-arrival.pipe");
    let temp = TempDir::new()?;
    let stored_path = temp.path().join("stored.jsonl");
    let audited_path = temp.path().join("audited.jsonl");

    cli()?
        .args([
            "audit",
            events.to_str().unwrap(),
            "--signal",
            "windowed-feature-sentiment",
            "--out",
            stored_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());

    cli()?
        .args([
            "audit",
            events.to_str().unwrap(),
            stored_path.to_str().unwrap(),
            "--signal",
            "windowed-feature-sentiment",
            "--out",
            audited_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());

    let audited = fs::read_to_string(&audited_path)?;
    let records: Vec<Value> = audited
        .lines()
        .map(|line| serde_json::from_str(line).expect("audit JSONL line should parse"))
        .collect();
    let expected_records = fs::read_to_string(&events)?
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.is_empty()
                && !line.starts_with('#')
                && line
                    .split('|')
                    .nth(4)
                    .is_some_and(|role| role == "prediction")
        })
        .count();

    assert_eq!(records.len(), expected_records);
    for record in records {
        assert_eq!(record["schema_version"], 2);
        assert_eq!(record["causally_valid"].as_bool(), Some(true));
        assert_eq!(record["matched_stored_prediction"].as_bool(), Some(true));
    }

    Ok(())
}

#[test]
fn audit_jsonl_validates_against_schema() -> TestResult {
    let output = success_output(&[
        "audit",
        "examples/late-arrival.pipe",
        "--signal",
        "windowed-feature-sentiment",
    ])?;
    let schema_path = repo_root().join("docs/audit.schema.json");
    let validator = schema_validator(&schema_path)?;
    let stdout = stdout_text(&output);
    let records = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("audit JSONL line should parse"))
        .collect::<Vec<_>>();

    assert!(!records.is_empty());
    for record in &records {
        validator
            .validate(record)
            .map_err(|error| format!("audit record failed schema validation: {error}"))?;
    }

    Ok(())
}

#[test]
fn negative_control_reports_three_leaks_for_synthetic_fixture() -> TestResult {
    cli()?
        .args([
            "negative-control",
            "examples/lookahead-negative-control.pipe",
            "--signal",
            "windowed-feature-sentiment",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("ENGINE A: received-time replay"))
        .stdout(predicate::str::contains("  impossible           0"))
        .stdout(predicate::str::contains("ENGINE B: observed-time replay"))
        .stdout(predicate::str::contains("  impossible           3"))
        .stdout(predicate::str::contains(
            "the broken engine emitted 3 impossible predictions across 3 distinct leak classes",
        ))
        .stderr(predicate::str::is_empty());

    Ok(())
}

#[test]
fn negative_control_reports_four_leaks_for_alfred_fixture() -> TestResult {
    cli()?
        .args([
            "negative-control",
            "examples/alfred-dgs10-sp500.pipe",
            "--signal",
            "windowed-zscore",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("ENGINE A: received-time replay"))
        .stdout(predicate::str::contains("  impossible           0"))
        .stdout(predicate::str::contains("ENGINE B: observed-time replay"))
        .stdout(predicate::str::contains("  impossible           4"))
        .stdout(predicate::str::contains(
            "the broken engine emitted 4 impossible predictions",
        ))
        .stderr(predicate::str::is_empty());

    Ok(())
}

#[test]
fn check_exits_nonzero_when_invariant_fails() -> TestResult {
    let temp = TempDir::new()?;
    let fixture = temp.path().join("no-contrast.pipe");
    fs::write(
        &fixture,
        "\
f1|100|100|1|feature|XYZ|sentiment=positive
p1|110|110|2|prediction|XYZ|
",
    )?;

    cli()?
        .args(["check", fixture.to_str().unwrap(), "--exhaustive"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("[FAIL]  on_time_vs_late_contrast"))
        .stderr(predicate::str::contains(
            "error: one or more adversarial checks failed",
        ));

    Ok(())
}

#[test]
fn run_suite_writes_expected_artifacts_and_valid_manifest() -> TestResult {
    let temp = TempDir::new()?;
    let out_dir = temp.path().join("suite");

    cli()?
        .args([
            "run-suite",
            "--scenario",
            "late-heavy",
            "--events",
            "16",
            "--symbols",
            "3",
            "--seed",
            "42",
            "--out",
            out_dir.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("asof-causality run-suite"))
        .stdout(predicate::str::contains("ARTIFACTS"))
        .stdout(predicate::str::contains("manifest.json"))
        .stderr(predicate::str::is_empty());

    for artifact in [
        "events.pipe",
        "predictions.pipe",
        "checks.txt",
        "summary.md",
        "manifest.json",
    ] {
        assert!(
            out_dir.join(artifact).is_file(),
            "missing run-suite artifact {artifact}"
        );
    }

    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(out_dir.join("manifest.json"))?)?;
    validate_json(&repo_root().join("docs/manifest.schema.json"), &manifest)?;
    assert_eq!(manifest["schema_version"], 3);
    assert_eq!(manifest["checks_passed"].as_bool(), Some(true));

    Ok(())
}

#[test]
fn generate_same_seed_is_byte_identical() -> TestResult {
    let args = [
        "generate",
        "--scenario",
        "late-heavy",
        "--events",
        "32",
        "--symbols",
        "4",
        "--seed",
        "42",
    ];
    let left = success_output(&args)?;
    let right = success_output(&args)?;

    assert_eq!(left.stdout, right.stdout);
    Ok(())
}

#[test]
fn argument_errors_use_stderr_without_stdout_data() -> TestResult {
    cli()?
        .args(["replay", "--bogus"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "error: unknown replay argument: --bogus",
        ));

    cli()?
        .args(["check", "--signal"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("error: --signal requires a value"));

    Ok(())
}

#[test]
fn unknown_top_level_command_prints_help_to_stdout() -> TestResult {
    cli()?
        .arg("not-a-command")
        .assert()
        .success()
        .stdout(predicate::str::contains("usage:"))
        .stdout(predicate::str::contains("asof-causality negative-control"))
        .stderr(predicate::str::is_empty());

    Ok(())
}

#[test]
fn replay_stdout_snapshot() -> TestResult {
    let output = success_output(&["replay", "examples/late-arrival.pipe"])?;
    insta::assert_snapshot!(stdout_text(&output), @r###"replay path=examples/late-arrival.pipe signal=last-feature-sentiment events=7
prediction_replay_key|symbol|signal_value|input_event_ids|max_input_replay_key
580:3:p1|AAPL|0|-|-
590:4:p2|AAPL|1|n1|585:2:n1
610:5:p3|AAPL|1|n1|585:2:n1
620:7:p4|AAPL|-1|c1|615:6:c1
transcript_hash=d959650f0492c42e
outcomes_seen=1
"###);

    Ok(())
}

#[test]
fn check_stdout_snapshot() -> TestResult {
    let output = success_output(&["check", "examples/late-arrival.pipe", "--exhaustive"])?;
    insta::assert_snapshot!(stdout_text(&output), @r###"asof-causality check
  fixture    examples/late-arrival.pipe
  events     7
  signal     last-feature-sentiment
  cutoffs    exhaustive (4)

ADVERSARIAL CHECKS                                         8/8 PASS
  [PASS]  prefix_equivalence               all received-time prefixes matched full replay
  [PASS]  future_mutation                  mutating future rows did not change past predictions
  [PASS]  late_arrival                     late events were not used before their replay key
  [PASS]  on_time_vs_late_contrast         moving n1 earlier changed prediction at 580 from 0 to 1
  [PASS]  feature_correction_append_only   feature corrections did not rewrite predictions emitted before receipt
  [PASS]  outcome_separation               disabling outcomes did not change predictions
  [PASS]  deterministic_replay             shuffled input produced same transcript hash
  [PASS]  audit_invariant                  all predictions satisfy max_input_replay_key <= prediction_replay_key

PROVENANCE
  transcript_hash      d959650f0492c42e
  predictions_emitted  4
  outcomes_separated   1
"###);

    Ok(())
}

#[test]
fn negative_control_stdout_snapshot() -> TestResult {
    let output = success_output(&[
        "negative-control",
        "examples/lookahead-negative-control.pipe",
        "--signal",
        "windowed-feature-sentiment",
    ])?;
    insta::assert_snapshot!(stdout_text(&output), @r###"asof-causality negative-control
  fixture  examples/lookahead-negative-control.pipe
  events   12
  signal   windowed-feature-sentiment

ENGINE A: received-time replay (correct)
  ordering             (received_time, sequence, event_id)
  transcript_hash      ed03706f6f79c31f
  impossible           0
  VERDICT              PASS

ENGINE B: observed-time replay (deliberately broken baseline)
  ordering             (observed_time, sequence, event_id)
  transcript_hash      f7b67d321cac694e
  impossible           3
  VERDICT              FAIL

LEAKED PREDICTIONS (engine B)

  p_before_same_time_sequence at (95, 4, p_before_same_time_sequence)
    signal_value     0
    leaked_input     n_same_time_later  at (95, 5, n_same_time_later)
    violation        input sequence > prediction sequence at same received_time
    interpretation   prediction at t=95 used same-timestamp event that sorts after it

  p_before_late_feature at (120, 6, p_before_late_feature)
    signal_value     1
    leaked_input     n_late_positive    at (150, 7, n_late_positive)
    violation        input replay key > prediction replay key by delta=30
    interpretation   prediction at t=120 used event that arrived at t=150

  p_before_correction at (170, 10, p_before_correction)
    signal_value     1
    leaked_input     c_late_negative    at (180, 9, c_late_negative)
    violation        input replay key > prediction replay key by delta=10
    interpretation   prediction at t=170 used correction received at t=180

DIAGNOSTIC
  the broken engine emitted 3 impossible predictions across 3 distinct leak classes
  the correct engine emitted 0
  the audit invariant catches the failure mode the engine is designed to prevent
"###);

    Ok(())
}
