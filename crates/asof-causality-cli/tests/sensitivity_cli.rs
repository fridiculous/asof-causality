use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn schema_validator(path: &Path) -> jsonschema::Validator {
    let schema: Value =
        serde_json::from_str(&fs::read_to_string(path).expect("schema should be readable"))
            .expect("schema should parse");
    jsonschema::validator_for(&schema).expect("schema should compile")
}

fn validate_json(schema_path: &Path, value: &Value) {
    let validator = schema_validator(schema_path);
    validator.validate(value).unwrap_or_else(|error| {
        panic!(
            "{} failed schema validation against {}",
            error,
            schema_path.display()
        )
    });
}

#[test]
fn sensitivity_cli_writes_summary_details_and_manifest() {
    let bin = env!("CARGO_BIN_EXE_asof-causality");
    let repo_root = repo_root();
    let events = repo_root.join("examples/alfred-dgs10-sp500.pipe");
    let out_dir = std::env::temp_dir().join(format!("asof-sensitivity-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out_dir);

    let output = Command::new(bin)
        .args([
            "sensitivity",
            events.to_str().unwrap(),
            "--signal",
            "windowed-zscore",
            "--shift-features",
            "-5000",
            "--shift-features",
            "-10000",
            "--observed-time-leaky",
            "--details",
            "--out",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("sensitivity command should run");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let summary_path = out_dir.join("summary.jsonl");
    let details_path = out_dir.join("details.jsonl");
    let manifest_path = out_dir.join("manifest.json");
    let sensitivity_curve_svg_path = out_dir.join("sensitivity-curve.svg");
    let flip_rate_svg_path = out_dir.join("flip-rate.svg");
    let input_change_svg_path = out_dir.join("input-change.svg");
    assert!(summary_path.is_file());
    assert!(details_path.is_file());
    assert!(manifest_path.is_file());
    assert!(sensitivity_curve_svg_path.is_file());
    assert!(flip_rate_svg_path.is_file());
    assert!(input_change_svg_path.is_file());
    assert!(!out_dir.join("late-arrival-impact.svg").exists());

    let summary = fs::read_to_string(&summary_path).expect("summary should be readable");
    let rows: Vec<Value> = summary
        .lines()
        .map(|line| serde_json::from_str(line).expect("summary row should parse"))
        .collect();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["policy_name"], "strict_received_time");
    assert_eq!(rows[1]["policy_name"], "shift_features_minus_5000");
    assert_eq!(rows[2]["policy_name"], "shift_features_minus_10000");
    assert_eq!(rows[3]["policy_name"], "observed_time_leaky");
    assert_eq!(rows[3]["category"], "realistic_failure");
    assert!(rows[3]["new_input_uses"].as_u64().unwrap() > 0);
    assert_eq!(rows[1]["policy"]["time_axis"], "fixture_native_integer");
    assert_eq!(rows[1]["policy"]["calendar_aware"], false);
    assert!(rows[1].get("new_inputs_admitted").is_none());
    assert!(rows[1].get("feature_recipe_hashes_changed").is_none());

    let summary_schema = repo_root.join("docs/sensitivity.summary.schema.json");
    for row in &rows {
        validate_json(&summary_schema, row);
    }

    let details = fs::read_to_string(&details_path).expect("details should be readable");
    assert!(!details.trim().is_empty());
    let detail_schema = repo_root.join("docs/sensitivity.detail.schema.json");
    for line in details.lines() {
        let detail: Value = serde_json::from_str(line).expect("detail row should parse");
        validate_json(&detail_schema, &detail);
    }

    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(&manifest_path).expect("manifest should be readable"),
    )
    .expect("manifest should parse");
    assert_eq!(manifest["schema_version"], "sensitivity-v1");
    assert_eq!(manifest["signal"], "windowed-zscore");
    assert_eq!(
        manifest["timestamp_shift_time_axis"],
        "fixture_native_integer"
    );
    assert_eq!(
        manifest["timestamp_shift_semantics"],
        "raw signed integer arithmetic on received_time; no calendar validation"
    );
    assert_eq!(manifest["policies"].as_array().unwrap().len(), 4);
    assert_eq!(manifest["details_path"], details_path.display().to_string());
    assert_eq!(
        manifest["sensitivity_curve_svg_path"],
        sensitivity_curve_svg_path.display().to_string()
    );
    assert_eq!(
        manifest["flip_rate_svg_path"],
        flip_rate_svg_path.display().to_string()
    );
    assert_eq!(
        manifest["input_change_svg_path"],
        input_change_svg_path.display().to_string()
    );
    assert_eq!(manifest["late_arrival_impact_svg_path"], Value::Null);
    assert_eq!(manifest["late_arrival_impact_svg_hash"], Value::Null);
    assert!(
        manifest["sensitivity_curve_svg_hash"]
            .as_str()
            .unwrap()
            .len()
            == 64
    );
    assert!(manifest["flip_rate_svg_hash"].as_str().unwrap().len() == 64);
    assert!(manifest["input_change_svg_hash"].as_str().unwrap().len() == 64);
    validate_json(
        &repo_root.join("docs/sensitivity.manifest.schema.json"),
        &manifest,
    );

    let sensitivity_curve_svg =
        fs::read_to_string(&sensitivity_curve_svg_path).expect("curve svg should read");
    assert!(sensitivity_curve_svg.contains("<svg"));
    assert!(sensitivity_curve_svg.contains("Sensitivity Curve"));
    assert!(sensitivity_curve_svg.contains("sampled x-values"));
    assert!(sensitivity_curve_svg.contains("baseline"));
    assert!(sensitivity_curve_svg.contains("observed-time policy reference"));

    let flip_rate_svg = fs::read_to_string(&flip_rate_svg_path).expect("flip svg should read");
    assert!(flip_rate_svg.contains("<svg"));
    assert!(flip_rate_svg.contains("Sensitivity Flip Rate"));
    assert!(flip_rate_svg.contains("observed_time_leaky"));

    let _ = fs::remove_dir_all(out_dir);
}

#[test]
fn sensitivity_cli_accepts_normalized_lookahead_range() {
    let bin = env!("CARGO_BIN_EXE_asof-causality");
    let repo_root = repo_root();
    let events = repo_root.join("examples/late-arrival.pipe");
    let out_dir =
        std::env::temp_dir().join(format!("asof-sensitivity-lookahead-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out_dir);

    let output = Command::new(bin)
        .args([
            "sensitivity",
            events.to_str().unwrap(),
            "--signal",
            "last-feature-sentiment",
            "--lookahead-range",
            "0..100",
            "--steps",
            "4",
            "--observed-time-leaky",
            "--out",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("sensitivity command should run");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let summary_path = out_dir.join("summary.jsonl");
    let manifest_path = out_dir.join("manifest.json");
    let curve_path = out_dir.join("sensitivity-curve.svg");
    let summary_schema = repo_root.join("docs/sensitivity.summary.schema.json");
    let rows: Vec<Value> = fs::read_to_string(&summary_path)
        .expect("summary should be readable")
        .lines()
        .map(|line| serde_json::from_str(line).expect("summary row should parse"))
        .collect();

    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0]["policy_name"], "strict_received_time");
    assert_eq!(rows[1]["policy_name"], "lookahead_25pct");
    assert_eq!(rows[4]["policy_name"], "lookahead_100pct");
    assert_eq!(rows[1]["policy"]["kind"], "received_time_lag_fraction");
    assert_eq!(rows[1]["policy"]["lag_fraction_bps"], 2500);
    assert_eq!(
        rows[1]["policy"]["shift_units"],
        "percent_of_each_event_lag"
    );
    assert_eq!(rows[1]["policy"]["bounded_by_observed_time"], true);
    for row in &rows {
        validate_json(&summary_schema, row);
    }

    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(&manifest_path).expect("manifest should be readable"),
    )
    .expect("manifest should parse");
    validate_json(
        &repo_root.join("docs/sensitivity.manifest.schema.json"),
        &manifest,
    );

    let curve = fs::read_to_string(&curve_path).expect("curve should be readable");
    assert!(curve.contains("feature publication lag removed (%)"));
    assert!(curve.contains("100%"));

    let _ = fs::remove_dir_all(out_dir);
}

#[test]
fn sensitivity_cli_accepts_late_arrival_scenario() {
    let bin = env!("CARGO_BIN_EXE_asof-causality");
    let repo_root = repo_root();
    let events = repo_root.join("examples/lookahead-negative-control.pipe");
    let out_dir =
        std::env::temp_dir().join(format!("asof-sensitivity-late-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out_dir);

    let output = Command::new(bin)
        .args([
            "sensitivity",
            events.to_str().unwrap(),
            "--signal",
            "windowed-feature-sentiment",
            "--scenario",
            "late-arrivals",
            "--out",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("sensitivity command should run");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let summary_path = out_dir.join("summary.jsonl");
    let manifest_path = out_dir.join("manifest.json");
    let late_svg_path = out_dir.join("late-arrival-impact.svg");
    let curve_path = out_dir.join("sensitivity-curve.svg");
    let rows: Vec<Value> = fs::read_to_string(&summary_path)
        .expect("summary should be readable")
        .lines()
        .map(|line| serde_json::from_str(line).expect("summary row should parse"))
        .collect();
    assert!(late_svg_path.is_file());
    assert!(!curve_path.exists());
    assert_eq!(rows[0]["policy_name"], "strict_received_time");
    assert!(rows
        .iter()
        .skip(1)
        .all(|row| row["policy"]["kind"] == "received_time_lag_bucket_lookahead"));

    let summary_schema = repo_root.join("docs/sensitivity.summary.schema.json");
    for row in &rows {
        validate_json(&summary_schema, row);
    }

    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(&manifest_path).expect("manifest should be readable"),
    )
    .expect("manifest should parse");
    assert_eq!(manifest["sensitivity_curve_svg_path"], Value::Null);
    assert_eq!(manifest["sensitivity_curve_svg_hash"], Value::Null);
    assert_eq!(
        manifest["late_arrival_impact_svg_path"],
        late_svg_path.display().to_string()
    );
    assert!(
        manifest["late_arrival_impact_svg_hash"]
            .as_str()
            .unwrap()
            .len()
            == 64
    );
    validate_json(
        &repo_root.join("docs/sensitivity.manifest.schema.json"),
        &manifest,
    );

    let late_svg = fs::read_to_string(&late_svg_path).expect("late svg should be readable");
    assert!(late_svg.contains("Late Arrival Impact"));

    let _ = fs::remove_dir_all(out_dir);
}
