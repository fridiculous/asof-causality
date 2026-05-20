use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn audit_cli_round_trips_stored_prediction_jsonl() {
    let bin = env!("CARGO_BIN_EXE_asof-causality");
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let events = repo_root.join("examples/late-arrival.pipe");
    let temp = std::env::temp_dir();
    let suffix = std::process::id();
    let stored_path = temp.join(format!("asof-stored-{suffix}.jsonl"));
    let audited_path = temp.join(format!("asof-audited-{suffix}.jsonl"));

    let replay_output = Command::new(bin)
        .args([
            "audit",
            events.to_str().unwrap(),
            "--signal",
            "windowed-feature-sentiment",
            "--out",
            stored_path.to_str().unwrap(),
        ])
        .output()
        .expect("audit replay-only command should run");
    assert!(
        replay_output.status.success(),
        "{}",
        String::from_utf8_lossy(&replay_output.stderr)
    );

    let stored_output = Command::new(bin)
        .args([
            "audit",
            events.to_str().unwrap(),
            stored_path.to_str().unwrap(),
            "--signal",
            "windowed-feature-sentiment",
            "--out",
            audited_path.to_str().unwrap(),
        ])
        .output()
        .expect("audit stored-prediction command should run");
    assert!(
        stored_output.status.success(),
        "{}",
        String::from_utf8_lossy(&stored_output.stderr)
    );

    let audited = fs::read_to_string(&audited_path).expect("audit output should be written");
    let records: Vec<Value> = audited
        .lines()
        .map(|line| serde_json::from_str(line).expect("audit JSONL line should parse"))
        .collect();
    let expected_records = fs::read_to_string(&events)
        .expect("fixture should be readable")
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
        assert_eq!(record["causally_valid"], true);
        assert_eq!(record["matched_stored_prediction"], true);
    }

    let _ = fs::remove_file(stored_path);
    let _ = fs::remove_file(audited_path);
}
