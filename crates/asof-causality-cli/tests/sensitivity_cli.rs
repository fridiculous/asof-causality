use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn sensitivity_cli_writes_summary_details_and_manifest() {
    let bin = env!("CARGO_BIN_EXE_asof-causality");
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
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
    let flip_rate_svg_path = out_dir.join("flip-rate.svg");
    let input_change_svg_path = out_dir.join("input-change.svg");
    assert!(summary_path.is_file());
    assert!(details_path.is_file());
    assert!(manifest_path.is_file());
    assert!(flip_rate_svg_path.is_file());
    assert!(input_change_svg_path.is_file());

    let summary = fs::read_to_string(&summary_path).expect("summary should be readable");
    let rows: Vec<Value> = summary
        .lines()
        .map(|line| serde_json::from_str(line).expect("summary row should parse"))
        .collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["policy_name"], "strict_received_time");
    assert_eq!(rows[1]["policy_name"], "shift_features_minus_10000");
    assert_eq!(rows[2]["policy_name"], "observed_time_leaky");
    assert_eq!(rows[2]["category"], "realistic_failure");
    assert!(rows[2]["new_input_uses"].as_u64().unwrap() > 0);

    let details = fs::read_to_string(&details_path).expect("details should be readable");
    assert!(!details.trim().is_empty());
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(&manifest_path).expect("manifest should be readable"),
    )
    .expect("manifest should parse");
    assert_eq!(manifest["schema_version"], "sensitivity-v1");
    assert_eq!(manifest["signal"], "windowed-zscore");
    assert_eq!(manifest["policies"].as_array().unwrap().len(), 3);
    assert_eq!(manifest["details_path"], details_path.display().to_string());
    assert_eq!(
        manifest["flip_rate_svg_path"],
        flip_rate_svg_path.display().to_string()
    );
    assert_eq!(
        manifest["input_change_svg_path"],
        input_change_svg_path.display().to_string()
    );
    assert!(manifest["flip_rate_svg_hash"].as_str().unwrap().len() == 64);
    assert!(manifest["input_change_svg_hash"].as_str().unwrap().len() == 64);

    let flip_rate_svg = fs::read_to_string(&flip_rate_svg_path).expect("flip svg should read");
    assert!(flip_rate_svg.contains("<svg"));
    assert!(flip_rate_svg.contains("Sensitivity Flip Rate"));
    assert!(flip_rate_svg.contains("observed_time_leaky"));

    let _ = fs::remove_dir_all(out_dir);
}
