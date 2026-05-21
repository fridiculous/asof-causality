#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Signal-validation workflow wrapper for the asof CLI.

This is deliberately orchestration only: resolve a named dataset, call the Rust
CLI as a subprocess, summarize the audit JSONL, and optionally render the
negative-control leak examples in a form a quant can act on while developing a
signal.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT_DIR = ROOT / "runs" / "demo"

DATASETS = {
    "macro-research-v1": {
        "title": "ALFRED DGS10 vintages with SP500 close predictions",
        "events": ROOT / "examples" / "alfred-dgs10-sp500.pipe",
        "outcomes": ROOT / "examples" / "alfred-dgs10-sp500.pipe",
    }
}


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run a quant-facing asof signal-validation demo."
    )
    parser.add_argument("--dataset", default="macro-research-v1", choices=sorted(DATASETS))
    parser.add_argument("--signal", default="windowed-zscore")
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR)
    parser.add_argument("--simulate-leak", action="store_true")
    parser.add_argument("--cargo", type=Path, help="path to cargo; defaults to PATH lookup")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    dataset = DATASETS[args.dataset]
    out_dir = args.out_dir.resolve()
    audit_path = out_dir / "audit.jsonl"
    manifest_path = out_dir / "manifest.json"
    cargo = str(args.cargo) if args.cargo else find_cargo()

    if cargo is None and not args.dry_run:
        print("error: cargo was not found on PATH; pass --cargo /path/to/cargo", file=sys.stderr)
        return 127
    cargo = cargo or "cargo"

    audit_cmd = [
        cargo,
        "run",
        "-p",
        "asof-cli",
        "--",
        "audit",
        rel(dataset["events"]),
        "--signal",
        args.signal,
        "--out",
        str(audit_path),
        "--outcomes",
        rel(dataset["outcomes"]),
    ]
    leak_cmd = [
        cargo,
        "run",
        "-p",
        "asof-cli",
        "--",
        "negative-control",
        rel(dataset["events"]),
        "--signal",
        args.signal,
    ]

    print("asof signal validation demo")
    print(f"  dataset   {args.dataset}")
    print(f"  signal    {args.signal}")
    print(f"  events    {rel(dataset['events'])}")
    print(f"  out       {display_path(out_dir)}")
    print()

    if args.dry_run:
        print("Commands")
        print(f"  {' '.join(audit_cmd)}")
        if args.simulate_leak:
            print(f"  {' '.join(leak_cmd)}")
        return 0

    out_dir.mkdir(parents=True, exist_ok=True)
    audit_result = run(audit_cmd)
    if audit_result.returncode != 0:
        print(audit_result.stdout, end="")
        print(audit_result.stderr, end="", file=sys.stderr)
        return audit_result.returncode

    records = read_jsonl(audit_path)
    manifest = write_manifest(manifest_path, args.dataset, args.signal, dataset, audit_path, audit_cmd)
    print_audit_summary(records, audit_path, manifest_path, manifest)

    if args.simulate_leak:
        leak_result = run(leak_cmd)
        if leak_result.returncode != 0:
            print(leak_result.stdout, end="")
            print(leak_result.stderr, end="", file=sys.stderr)
            return leak_result.returncode
        print_leak_examples(dataset["events"], leak_result.stdout)

    return 0


def find_cargo() -> str | None:
    found = shutil.which("cargo")
    if found:
        return found
    for candidate in (
        Path.home() / ".cargo" / "bin" / "cargo",
        Path("/opt/homebrew/bin/cargo"),
        Path("/usr/local/bin/cargo"),
    ):
        if candidate.exists():
            return str(candidate)
    return None


def run(command: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    rows = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        if not line.strip():
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError as error:
            raise ValueError(f"{path}:{line_number}: invalid JSONL: {error}") from error
    return rows


def write_manifest(
    path: Path,
    dataset_name: str,
    signal: str,
    dataset: dict[str, Path | str],
    audit_path: Path,
    command: list[str],
) -> dict[str, Any]:
    manifest = {
        "schema_version": 1,
        "kind": "signal_validation_demo",
        "generated_at": datetime.now(timezone.utc).replace(microsecond=0).isoformat(),
        "dataset": dataset_name,
        "dataset_title": dataset["title"],
        "signal": signal,
        "events_path": rel(dataset["events"]),
        "events_sha256": sha256_file(dataset["events"]),
        "audit_path": display_path(audit_path),
        "audit_sha256": sha256_file(audit_path),
        "hash_algorithm": "sha256",
        "command": command,
        "note": "Wrapper-level signal-validation demo manifest; run-suite writes the kernel run certificate.",
    }
    path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest


def print_audit_summary(
    records: list[dict[str, Any]],
    audit_path: Path,
    manifest_path: Path,
    manifest: dict[str, Any],
) -> None:
    total = len(records)
    non_causal = sum(1 for row in records if not row.get("causally_valid", False))
    outcomes = sum(1 for row in records if row.get("outcome") is not None)
    matches = [row.get("matched_stored_prediction") for row in records]
    supplied_matches = [value for value in matches if value is not None]
    mismatches = sum(1 for value in supplied_matches if value is False)
    status = "PASS" if non_causal == 0 and mismatches == 0 else "FAIL"

    print(f"Signal causality check: {status}")
    print(f"  Predictions audited       {total}")
    print(f"  Non-causal                {non_causal}")
    print(f"  Outcomes attached         {outcomes}")
    if supplied_matches:
        matched = sum(1 for value in supplied_matches if value is True)
        print(f"  Stored predictions matched {matched}/{len(supplied_matches)}")
    else:
        print("  Stored predictions matched not supplied")
    print(f"  Events SHA-256            {manifest['events_sha256'][:16]}...")
    print(f"  Audit SHA-256             {manifest['audit_sha256'][:16]}...")
    print()
    print("Artifacts")
    print(f"  audit JSONL               {display_path(audit_path)}")
    print(f"  manifest                  {display_path(manifest_path)}")
    print()
    print("Interpretation")
    print("  This validates one signal-quality gate: as-of causality.")
    print("  It does not test predictive power, PnL, fills, costs, or strategy rules.")


def print_leak_examples(events_path: Path, negative_control_stdout: str) -> None:
    leaks = parse_leaks(negative_control_stdout)
    events = read_pipe_events(events_path)
    print()
    print("Leak simulation: observed-time baseline")
    if not leaks:
        print("  No impossible predictions were parsed from negative-control output.")
        return

    print(f"  Impossible predictions    {len(leaks)}")
    print()
    for index, leak in enumerate(leaks, start=1):
        prediction = events.get(leak["prediction_id"], {})
        leaked = events.get(leak["leaked_input_id"], {})
        print(f"Example {index}")
        print("  Prediction")
        print(f"    event        {leak['prediction_id']}")
        print(f"    replay key   {leak['prediction_key']}")
        if prediction:
            print(f"    observed     {prediction['observed_time']}")
            print(f"    received     {prediction['received_time']}")
            print(f"    symbol       {prediction['symbol']}")
        print(f"    signal       {leak['signal_value']}")
        print("  Leaked input")
        print(f"    event        {leak['leaked_input_id']}")
        print(f"    replay key   {leak['leaked_input_key']}")
        if leaked:
            print(f"    observed     {leaked['observed_time']}")
            print(f"    received     {leaked['received_time']}")
            print(f"    payload      {leaked['payload']}")
        print("  Problem")
        print(f"    {leak['violation']}")
        print(f"    {leak['interpretation']}")
        print("  Likely fix")
        print("    Use received-time/as-of joins and preserve vendor or ingestion availability.")
        print()


def parse_leaks(text: str) -> list[dict[str, str]]:
    leaks: list[dict[str, str]] = []
    current: dict[str, str] | None = None
    pred_re = re.compile(r"^\s{2}(\S+) at \(([^)]+)\)")
    input_re = re.compile(r"^\s+leaked_input\s+(\S+)\s+at \(([^)]+)\)")
    field_re = re.compile(r"^\s+([a-z_]+)\s+(.+)$")

    for line in text.splitlines():
        if match := pred_re.match(line):
            if current:
                append_complete_leak(leaks, current)
            current = {"prediction_id": match.group(1), "prediction_key": tuple_key(match.group(2))}
            continue
        if current is None:
            continue
        if match := input_re.match(line):
            current["leaked_input_id"] = match.group(1)
            current["leaked_input_key"] = tuple_key(match.group(2))
        elif match := field_re.match(line):
            current[match.group(1)] = match.group(2).strip()

    if current:
        append_complete_leak(leaks, current)
    return leaks


def append_complete_leak(leaks: list[dict[str, str]], leak: dict[str, str]) -> None:
    required = {
        "prediction_id",
        "prediction_key",
        "signal_value",
        "leaked_input_id",
        "leaked_input_key",
        "violation",
        "interpretation",
    }
    if required <= leak.keys():
        leaks.append(leak)


def read_pipe_events(path: Path) -> dict[str, dict[str, str]]:
    events = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        event_id, observed, received, sequence, role, symbol, payload = line.split("|", 6)
        events[event_id] = {
            "observed_time": observed,
            "received_time": received,
            "sequence": sequence,
            "role": role,
            "symbol": symbol,
            "payload": payload,
        }
    return events


def tuple_key(value: str) -> str:
    parts = [part.strip() for part in value.split(",", 2)]
    return f"{parts[0]}:{parts[1]}:{parts[2]}" if len(parts) == 3 else value


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def rel(path: Path) -> str:
    return str(path.relative_to(ROOT))


def display_path(path: Path) -> str:
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


if __name__ == "__main__":
    raise SystemExit(main())
