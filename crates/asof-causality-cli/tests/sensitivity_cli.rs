use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

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

fn run_sensitivity(args: &[String]) {
    let output = Command::new(env!("CARGO_BIN_EXE_asof-causality"))
        .args(args)
        .output()
        .expect("sensitivity command should run");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("json file should be readable"))
        .expect("json file should parse")
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("jsonl file should be readable")
        .lines()
        .map(|line| serde_json::from_str(line).expect("jsonl row should parse"))
        .collect()
}

fn file_hash(path: &Path) -> String {
    asof_causality_core::blake3_hex(&fs::read(path).expect("artifact should be readable"))
}

fn assert_artifact_hash(manifest: &Value, path_key: &str, hash_key: &str) {
    match manifest[path_key].as_str() {
        Some(path) => assert_eq!(
            manifest[hash_key].as_str().unwrap(),
            file_hash(Path::new(path)),
            "{hash_key} should match {path_key}"
        ),
        None => assert_eq!(
            manifest[hash_key],
            Value::Null,
            "{hash_key} should be null when {path_key} is null"
        ),
    }
}

fn assert_all_manifest_artifact_hashes(manifest: &Value) {
    assert_artifact_hash(manifest, "summary_path", "summary_hash");
    assert_artifact_hash(manifest, "details_path", "details_hash");
    assert_artifact_hash(
        manifest,
        "sensitivity_curve_svg_path",
        "sensitivity_curve_svg_hash",
    );
    assert_artifact_hash(manifest, "flip_rate_svg_path", "flip_rate_svg_hash");
    assert_artifact_hash(manifest, "input_change_svg_path", "input_change_svg_hash");
    assert_artifact_hash(
        manifest,
        "late_arrival_impact_svg_path",
        "late_arrival_impact_svg_hash",
    );
}

fn assert_manifest_policies_match_summary(manifest: &Value, summary_rows: &[Value]) {
    let summary_by_name = summary_rows
        .iter()
        .map(|row| (row["policy_name"].as_str().unwrap(), row))
        .collect::<BTreeMap<_, _>>();

    for policy in manifest["policies"].as_array().unwrap() {
        let name = policy["name"].as_str().unwrap();
        let summary = summary_by_name
            .get(name)
            .unwrap_or_else(|| panic!("manifest policy {name} should have a summary row"));
        assert_eq!(policy["category"], summary["category"]);
        assert_eq!(policy["descriptor"], summary["policy"]);
        assert_eq!(policy["events_transformed"], summary["events_transformed"]);
        assert_eq!(
            policy["transformed_fixture_hash"],
            summary["transformed_fixture_hash"]
        );
        assert_eq!(policy["transcript_hash"], summary["transcript_hash"]);
    }
}

fn assert_summary_matches_details(summary_rows: &[Value], details: &[Value]) {
    let mut details_by_policy: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for detail in details {
        let policy_name = detail["policy_name"].as_str().unwrap();
        let signal_changed = detail["signal_value_changed"].as_bool().unwrap();
        let recipe_changed = detail["feature_recipe_hash_changed"].as_bool().unwrap();
        let new_inputs = detail["new_inputs_admitted"].as_array().unwrap();
        assert!(
            signal_changed || recipe_changed || !new_inputs.is_empty(),
            "detail rows should only be emitted for affected predictions"
        );
        details_by_policy
            .entry(policy_name)
            .or_default()
            .push(detail);
    }

    for row in summary_rows {
        let predictions = row["predictions"].as_u64().unwrap();
        let signal_changes = row["predictions_with_signal_change"].as_u64().unwrap();
        let expected_flip_rate = if predictions == 0 {
            0.0
        } else {
            signal_changes as f64 / predictions as f64
        };
        let actual_flip_rate = row["flip_rate"].as_f64().unwrap();
        assert!(
            (actual_flip_rate - expected_flip_rate).abs() < f64::EPSILON,
            "flip_rate should be exactly derived from integer counts for {}",
            row["policy_name"]
        );

        if row["category"] == "baseline" {
            assert_eq!(row["predictions_with_signal_change"], 0);
            assert_eq!(row["predictions_with_recipe_change"], 0);
            assert_eq!(row["predictions_with_new_inputs"], 0);
            assert_eq!(row["new_input_uses"], 0);
            assert_eq!(row["unique_new_inputs"], 0);
            continue;
        }

        let policy_name = row["policy_name"].as_str().unwrap();
        let policy_details = details_by_policy
            .get(policy_name)
            .cloned()
            .unwrap_or_default();
        let signal_change_count = policy_details
            .iter()
            .filter(|detail| detail["signal_value_changed"].as_bool().unwrap())
            .count() as u64;
        let recipe_change_count = policy_details
            .iter()
            .filter(|detail| detail["feature_recipe_hash_changed"].as_bool().unwrap())
            .count() as u64;
        let predictions_with_new_inputs = policy_details
            .iter()
            .filter(|detail| !detail["new_inputs_admitted"].as_array().unwrap().is_empty())
            .count() as u64;
        let new_input_uses = policy_details
            .iter()
            .map(|detail| detail["new_inputs_admitted"].as_array().unwrap().len() as u64)
            .sum::<u64>();
        let unique_new_inputs = policy_details
            .iter()
            .flat_map(|detail| detail["new_inputs_admitted"].as_array().unwrap())
            .map(|input| input.as_str().unwrap())
            .collect::<BTreeSet<_>>()
            .len() as u64;

        assert_eq!(
            row["predictions_with_signal_change"], signal_change_count,
            "{policy_name} signal change count should match detail rows"
        );
        assert_eq!(
            row["predictions_with_recipe_change"], recipe_change_count,
            "{policy_name} recipe change count should match detail rows"
        );
        assert_eq!(
            row["predictions_with_new_inputs"], predictions_with_new_inputs,
            "{policy_name} new-input prediction count should match detail rows"
        );
        assert_eq!(
            row["new_input_uses"], new_input_uses,
            "{policy_name} new input uses should match detail rows"
        );
        assert_eq!(
            row["unique_new_inputs"], unique_new_inputs,
            "{policy_name} unique inputs should match detail rows"
        );
    }
}

fn tiny_late_arrival_fixture(dir: &Path) -> PathBuf {
    let fixture = dir.join("tiny-late-arrival.pipe");
    fs::write(
        &fixture,
        "\
# event_id|observed_time|received_time|received_sequence_number|role|symbol|payload
f_seed|10|10|1|feature|XYZ|sentiment=negative
p_early|50|50|2|prediction|XYZ|
n_late|40|80|3|feature|XYZ|sentiment=positive
p_late|90|90|4|prediction|XYZ|
",
    )
    .expect("tiny fixture should write");
    fixture
}

fn normalize_manifest_for_determinism(mut manifest: Value) -> Value {
    manifest["run_started_utc"] = json!("<normalized>");
    manifest["invocation"] = json!("<normalized>");
    manifest["invocation_args"] = json!(["<normalized>"]);
    for key in [
        "summary_path",
        "details_path",
        "sensitivity_curve_svg_path",
        "flip_rate_svg_path",
        "input_change_svg_path",
        "late_arrival_impact_svg_path",
    ] {
        if manifest[key].is_string() {
            manifest[key] = json!("<normalized-path>");
        }
    }
    manifest
}

fn svg_height(svg: &str) -> usize {
    let view_box = svg
        .split("viewBox=\"0 0 900 ")
        .nth(1)
        .expect("svg should use the expected viewBox width");
    view_box
        .split('"')
        .next()
        .unwrap()
        .parse()
        .expect("svg height should parse")
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
    assert!(flip_rate_svg.contains("Observed-time replay"));

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
        .all(|row| row["policy"]["kind"] == "received_time_lag_fraction"));

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
    assert!(late_svg.contains("Cumulative Late Arrival Exposure"));
    assert!(late_svg.contains("late-arrival lag removed"));
    assert!(late_svg.contains(
        "Each point replays all late feature/correction events shifted by that fraction"
    ));
    assert!(!late_svg.contains(">late_arrivals_lag_"));

    let _ = fs::remove_dir_all(out_dir);
}

#[test]
fn sensitivity_manifest_hashes_match_artifacts() {
    let repo_root = repo_root();
    let events = repo_root.join("examples/alfred-dgs10-sp500.pipe");
    let temp = TempDir::new().expect("tempdir should create");
    let out_dir = temp.path().join("hashes");

    run_sensitivity(&[
        "sensitivity".to_string(),
        events.display().to_string(),
        "--signal".to_string(),
        "windowed-zscore".to_string(),
        "--shift-features".to_string(),
        "-5000".to_string(),
        "--observed-time-leaky".to_string(),
        "--details".to_string(),
        "--out".to_string(),
        out_dir.display().to_string(),
    ]);

    let manifest = read_json(&out_dir.join("manifest.json"));
    let summary_rows = read_jsonl(&out_dir.join("summary.jsonl"));

    assert_all_manifest_artifact_hashes(&manifest);
    assert_manifest_policies_match_summary(&manifest, &summary_rows);
    assert!(manifest["sensitivity_curve_svg_path"].is_string());
    assert!(manifest["sensitivity_curve_svg_hash"].is_string());
    assert_eq!(manifest["late_arrival_impact_svg_path"], Value::Null);
    assert_eq!(manifest["late_arrival_impact_svg_hash"], Value::Null);
}

#[test]
fn sensitivity_summary_matches_details() {
    let temp = TempDir::new().expect("tempdir should create");
    let fixture = tiny_late_arrival_fixture(temp.path());
    let out_dir = temp.path().join("summary-details");

    run_sensitivity(&[
        "sensitivity".to_string(),
        fixture.display().to_string(),
        "--signal".to_string(),
        "last-feature-sentiment".to_string(),
        "--scenario".to_string(),
        "late-arrivals".to_string(),
        "--steps".to_string(),
        "2".to_string(),
        "--details".to_string(),
        "--out".to_string(),
        out_dir.display().to_string(),
    ]);

    let summary_rows = read_jsonl(&out_dir.join("summary.jsonl"));
    let details = read_jsonl(&out_dir.join("details.jsonl"));

    assert_summary_matches_details(&summary_rows, &details);
}

#[test]
fn sensitivity_tiny_late_arrival_fixture_has_exact_expected_rows() {
    let temp = TempDir::new().expect("tempdir should create");
    let fixture = tiny_late_arrival_fixture(temp.path());
    let out_dir = temp.path().join("tiny-late");

    run_sensitivity(&[
        "sensitivity".to_string(),
        fixture.display().to_string(),
        "--signal".to_string(),
        "last-feature-sentiment".to_string(),
        "--scenario".to_string(),
        "late-arrivals".to_string(),
        "--steps".to_string(),
        "2".to_string(),
        "--details".to_string(),
        "--out".to_string(),
        out_dir.display().to_string(),
    ]);

    let rows = read_jsonl(&out_dir.join("summary.jsonl"));
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["policy_name"], "strict_received_time");
    assert_eq!(rows[0]["new_input_uses"], 0);
    assert_eq!(rows[0]["predictions_with_signal_change"], 0);

    assert_eq!(rows[1]["policy_name"], "late_arrivals_cumulative_50pct");
    assert_eq!(rows[1]["policy"]["lag_fraction_bps"], 5000);
    assert_eq!(rows[1]["new_input_uses"], 0);
    assert_eq!(rows[1]["predictions_with_signal_change"], 0);
    assert_eq!(rows[1]["flip_rate"], 0.0);

    assert_eq!(rows[2]["policy_name"], "late_arrivals_cumulative_100pct");
    assert_eq!(rows[2]["policy"]["lag_fraction_bps"], 10000);
    assert_eq!(rows[2]["predictions"], 2);
    assert_eq!(rows[2]["predictions_with_signal_change"], 1);
    assert_eq!(rows[2]["predictions_with_recipe_change"], 1);
    assert_eq!(rows[2]["predictions_with_new_inputs"], 1);
    assert_eq!(rows[2]["new_input_uses"], 1);
    assert_eq!(rows[2]["unique_new_inputs"], 1);
    assert_eq!(rows[2]["flip_rate"], 0.5);

    let details = read_jsonl(&out_dir.join("details.jsonl"));
    assert_eq!(details.len(), 1);
    assert_eq!(details[0]["policy_name"], "late_arrivals_cumulative_100pct");
    assert_eq!(details[0]["prediction_event_id"], "p_early");
    assert_eq!(details[0]["new_inputs_admitted"], json!(["n_late"]));
    assert_eq!(
        details[0]["baseline"]["input_event_ids_used"],
        json!(["f_seed"])
    );
    assert_eq!(
        details[0]["comparison"]["input_event_ids_used"],
        json!(["n_late"])
    );
    assert_eq!(details[0]["signal_value_changed"], true);
    assert_eq!(details[0]["feature_recipe_hash_changed"], true);
}

#[test]
fn sensitivity_lookahead_fixture_has_exact_policy_steps() {
    let temp = TempDir::new().expect("tempdir should create");
    let fixture = tiny_late_arrival_fixture(temp.path());
    let out_dir = temp.path().join("tiny-lookahead");

    run_sensitivity(&[
        "sensitivity".to_string(),
        fixture.display().to_string(),
        "--signal".to_string(),
        "last-feature-sentiment".to_string(),
        "--lookahead-range".to_string(),
        "0..100".to_string(),
        "--steps".to_string(),
        "4".to_string(),
        "--details".to_string(),
        "--out".to_string(),
        out_dir.display().to_string(),
    ]);

    let rows = read_jsonl(&out_dir.join("summary.jsonl"));
    let names = rows
        .iter()
        .map(|row| row["policy_name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "strict_received_time",
            "lookahead_25pct",
            "lookahead_50pct",
            "lookahead_75pct",
            "lookahead_100pct"
        ]
    );

    for (row, bps) in rows.iter().skip(1).zip([2500, 5000, 7500, 10000]) {
        assert_eq!(row["policy"]["kind"], "received_time_lag_fraction");
        assert_eq!(row["policy"]["lag_fraction_bps"], bps);
        assert_eq!(row["policy"]["bounded_by_observed_time"], true);
        assert_eq!(row["events_transformed"], 1);
    }

    assert_eq!(rows[1]["predictions_with_signal_change"], 0);
    assert_eq!(rows[2]["predictions_with_signal_change"], 0);
    assert_eq!(rows[3]["predictions_with_signal_change"], 0);
    assert_eq!(rows[4]["predictions_with_signal_change"], 1);
    assert_eq!(rows[4]["new_input_uses"], 1);

    let details = read_jsonl(&out_dir.join("details.jsonl"));
    assert_eq!(details.len(), 1);
    assert_eq!(details[0]["policy_name"], "lookahead_100pct");
    assert_eq!(details[0]["prediction_event_id"], "p_early");
    assert_eq!(details[0]["new_inputs_admitted"], json!(["n_late"]));
}

#[test]
fn sensitivity_outputs_are_deterministic_across_runs() {
    let temp = TempDir::new().expect("tempdir should create");
    let fixture = tiny_late_arrival_fixture(temp.path());
    let out_a = temp.path().join("run-a");
    let out_b = temp.path().join("run-b");

    for out_dir in [&out_a, &out_b] {
        run_sensitivity(&[
            "sensitivity".to_string(),
            fixture.display().to_string(),
            "--signal".to_string(),
            "last-feature-sentiment".to_string(),
            "--scenario".to_string(),
            "late-arrivals".to_string(),
            "--steps".to_string(),
            "2".to_string(),
            "--details".to_string(),
            "--out".to_string(),
            out_dir.display().to_string(),
        ]);
    }

    for file in [
        "summary.jsonl",
        "details.jsonl",
        "flip-rate.svg",
        "input-change.svg",
        "late-arrival-impact.svg",
    ] {
        assert_eq!(
            fs::read_to_string(out_a.join(file)).unwrap(),
            fs::read_to_string(out_b.join(file)).unwrap(),
            "{file} should be deterministic"
        );
    }

    let manifest_a = normalize_manifest_for_determinism(read_json(&out_a.join("manifest.json")));
    let manifest_b = normalize_manifest_for_determinism(read_json(&out_b.join("manifest.json")));
    assert_eq!(manifest_a, manifest_b);
}

#[test]
fn sensitivity_svgs_have_expected_semantic_content() {
    let temp = TempDir::new().expect("tempdir should create");
    let fixture = tiny_late_arrival_fixture(temp.path());

    for (steps, max_height) in [(20, 650), (50, 1200)] {
        let out_dir = temp.path().join(format!("svg-{steps}"));
        run_sensitivity(&[
            "sensitivity".to_string(),
            fixture.display().to_string(),
            "--signal".to_string(),
            "last-feature-sentiment".to_string(),
            "--scenario".to_string(),
            "late-arrivals".to_string(),
            "--steps".to_string(),
            steps.to_string(),
            "--out".to_string(),
            out_dir.display().to_string(),
        ]);

        for file in [
            "flip-rate.svg",
            "input-change.svg",
            "late-arrival-impact.svg",
        ] {
            let svg = fs::read_to_string(out_dir.join(file)).unwrap();
            assert!(svg.starts_with("<svg "));
            assert!(svg.contains("<title"));
            assert!(svg.contains("<desc"));
            assert!(svg.ends_with("</svg>\n"));
            assert!(!svg.contains(">late_arrivals_lag_"));
        }

        let flip_svg = fs::read_to_string(out_dir.join("flip-rate.svg")).unwrap();
        let input_svg = fs::read_to_string(out_dir.join("input-change.svg")).unwrap();
        let late_svg = fs::read_to_string(out_dir.join("late-arrival-impact.svg")).unwrap();
        assert_eq!(flip_svg.matches("fill=\"#2563eb\"").count(), steps);
        assert_eq!(input_svg.matches("fill=\"#2563eb\"").count(), steps);
        assert_eq!(late_svg.matches("<circle ").count(), steps + 1);
        assert!(
            svg_height(&flip_svg) <= max_height,
            "flip-rate.svg should stay compact for {steps} rows"
        );
        assert!(
            svg_height(&input_svg) <= max_height,
            "input-change.svg should stay compact for {steps} rows"
        );
        assert!(late_svg.contains("Cumulative Late Arrival Exposure"));
        assert!(late_svg.contains("unique prediction/input admissions"));
        assert!(late_svg.contains("100.0%"));
    }
}

#[test]
fn sensitivity_schemas_cover_all_supported_signals() {
    let repo_root = repo_root();
    let temp = TempDir::new().expect("tempdir should create");
    let summary_schema = repo_root.join("docs/sensitivity.summary.schema.json");
    let manifest_schema = repo_root.join("docs/sensitivity.manifest.schema.json");
    let cases = [
        (
            "last-feature-sentiment",
            repo_root.join("examples/late-arrival.pipe"),
        ),
        (
            "windowed-feature-sentiment",
            repo_root.join("examples/late-arrival.pipe"),
        ),
        (
            "windowed-zscore",
            repo_root.join("examples/zscore-lookahead.pipe"),
        ),
        (
            "vol-adjusted-momentum",
            repo_root.join("examples/zscore-lookahead.pipe"),
        ),
    ];

    for (signal, fixture) in cases {
        let out_dir = temp.path().join(signal);
        run_sensitivity(&[
            "sensitivity".to_string(),
            fixture.display().to_string(),
            "--signal".to_string(),
            signal.to_string(),
            "--lookahead-range".to_string(),
            "0..100".to_string(),
            "--steps".to_string(),
            "1".to_string(),
            "--out".to_string(),
            out_dir.display().to_string(),
        ]);

        let summary_rows = read_jsonl(&out_dir.join("summary.jsonl"));
        for row in &summary_rows {
            assert_eq!(row["signal_name"], signal);
            validate_json(&summary_schema, row);
        }
        let manifest = read_json(&out_dir.join("manifest.json"));
        assert_eq!(manifest["signal"], signal);
        validate_json(&manifest_schema, &manifest);
    }
}
