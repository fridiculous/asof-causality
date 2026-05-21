# asof-causality

asof-causality is a deterministic falsification harness for temporal leakage.
It tests whether a historical prediction only used data that was knowable at
the exact replay key when the prediction was made.

Most backtests ask, "Did the signal work?" This repo asks the prior systems
question: "Could the signal or training-data pipeline have known what it used
at that time?" Temporal leakage shows up in backtests, time-series training
data, and AI-assisted research workflows whenever historical code can see rows
that were not actually available yet.

The engine enforces a two-clock event model, restricts signal code to an opaque
as-of view, records immutable prediction logs with input provenance, and ships
a negative control that shows the exact impossible predictions a naive
observed-time replay would emit. It evaluates causality, not predictive alpha.

## 30-Second Demo

```sh
cargo run -p asof-causality-cli -- negative-control examples/lookahead-negative-control.pipe --signal windowed-feature-sentiment
```

Expected diagnostic:

```text
asof-causality negative-control
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
```

The correct engine orders by `(received_time, sequence, event_id)`. The broken
baseline orders by `(observed_time, sequence, event_id)`, so it leaks a
same-timestamp later sequence, a late feature, and a late correction into
predictions that could not have used them in live replay.

## What This Builds

- a Rust workspace with a reusable core crate and CLI
- a pipe-delimited event format with `observed_time` and `received_time`
- canonical event roles: `feature`, `feature_correction`, `prediction`, and
  `outcome`
- deterministic replay by `(received_time, sequence, event_id)`
- a restricted signal API that receives only an opaque `AsOfView`
- built-in single-input, windowed multi-input, and numeric Z-score signals
- immutable `PredictionRecord` output with input-event provenance
- JSONL audit output with its schema documented in `docs/audit.schema.json`
- sensitivity summary/detail/manifest contracts documented in
  `docs/sensitivity.*.schema.json`
- interned symbol IDs in replay state and prediction records, rendered back to
  human symbols in transcripts
- adversarial leakage checks for late arrivals, feature corrections, outcomes, and
  shuffled physical input
- a `manifest.json` run certificate that links inputs, outputs, checks, signal
  version, invocation, toolchain, and transcript hash
- a synthetic throughput benchmark comparing string-keyed state with interned
  symbol IDs

The kernel is signal-agnostic. The built-ins include deliberately simple
sentiment signals and a numeric `windowed-zscore` signal over continuous
`score=...` payloads. The non-trivial part is the correctness cage around the
signal: all of them receive only an opaque as-of view and emit provenance.

## Quick Start

Install Rust 1.78+ if needed, then:

```sh
cargo run -p asof-causality-cli -- replay examples/late-arrival.pipe
cargo run -p asof-causality-cli -- check examples/late-arrival.pipe
cargo run -p asof-causality-cli -- audit examples/late-arrival.pipe
cargo run -p asof-causality-cli -- negative-control examples/lookahead-negative-control.pipe
cargo run -p asof-causality-cli -- negative-control examples/lookahead-negative-control.pipe --signal windowed-feature-sentiment
cargo run -p asof-causality-cli -- negative-control examples/zscore-lookahead.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- check examples/alfred-dgs10-sp500.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- check examples/alfred-payems-revision.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- sensitivity examples/alfred-dgs10-sp500.pipe --signal windowed-zscore --scenario lookahead --lookahead-range 0..100 --steps 4 --details --out runs/alfred-sensitivity
cargo run -p asof-causality-cli -- sensitivity examples/lookahead-negative-control.pipe --signal windowed-feature-sentiment --scenario late-arrivals --out runs/late-arrival-sensitivity
make verify-real-data-demo
make verify-real-revision-demo
cargo run -p asof-causality-cli -- generate --scenario late-heavy --events 100000 --symbols 1024 --late-rate 0.30 --feature-correction-rate 0.05 --seed 42 --out runs/late-heavy.pipe
cargo run -p asof-causality-cli -- run-suite --scenario late-heavy --events 100000 --symbols 1024 --seed 42 --out runs/late-heavy
cargo run -p asof-causality-cli -- bench --events 1000000 --symbols 1024
cargo test
```

This repository has no runtime network dependency. Most fixtures are synthetic;
`examples/alfred-dgs10-sp500.pipe` is a small checked-in real-data fixture
derived from public ALFRED/FRED CSV downloads. `make verify-real-data-demo`
rebuilds that fixture from the source CSVs and checks it byte-for-byte.
`examples/alfred-payems-revision.pipe` is a separate ALFRED-only fixture with a
real PAYEMS revision; `make verify-real-revision-demo` rebuilds it from the
source vintages. Those verification commands require internet access to the
public CSV endpoints, but they do not require an API key, an LLM key, CUDA, or a
database.

## Event Format

```text
event_id|observed_time|received_time|sequence|role|symbol|payload
```

Events are replayed by `(received_time, sequence, event_id)`, not by physical
file order or observed time. A late event may influence future predictions, but
it cannot mutate old predictions.

The canonical roles are:

| Role | Meaning |
|---|---|
| `feature` | Source information the signal may use after `received_time` |
| `feature_correction` | Append-only revision to earlier feature information |
| `prediction` | Scheduled point where the signal emits a prediction |
| `outcome` | Future evaluation data excluded from signal state |

## Current Commands

```sh
cargo run -p asof-causality-cli -- replay examples/late-arrival.pipe
```

Prints deterministic prediction records and a transcript hash.

Use `--signal windowed-feature-sentiment` to run the same replay through the
bounded multi-input signal, or `--signal windowed-zscore` for numeric
`score=...` features. The default is `last-feature-sentiment`.

```sh
cargo run -p asof-causality-cli -- check examples/late-arrival.pipe
```

Runs the adversarial replay suite against the fixture.

For large generated files, `check` samples 32 received-time cutoffs by default
for the expensive prefix-equivalence and future-mutation checks. Use
`--exhaustive` for the full adversarial sweep on small fixtures, or
`--max-cutoffs N` to set the deterministic cutoff sample size.

```sh
cargo run -p asof-causality-cli -- audit examples/late-arrival.pipe --signal windowed-feature-sentiment
```

Emits one JSON object per replay-derived prediction. Each record includes the
prediction replay key, signal, symbol, signal value, ordered input event IDs,
optional maximum input replay key, BLAKE3 `feature_recipe_hash`,
`causally_valid`, optional `matched_stored_prediction`, and optional `outcome`.
The machine-readable contract lives in
[docs/audit.schema.json](docs/audit.schema.json). Use `--out path` to write the
same JSONL stream to a file.

To audit stored predictions instead of only emitting the replay-derived audit
surface:

```sh
cargo run -p asof-causality-cli -- audit events.pipe stored_predictions.jsonl outcomes.pipe --out audit.jsonl
```

Stored predictions are matched by `(symbol, prediction_replay_key)` and should
include `signal_value` plus `feature_recipe_hash`. Use
`--allow-missing-recipe-hash` only for legacy stored predictions that can be
matched on `signal_value` alone. Outcomes are attached only when they explicitly
name `prediction_replay_key`; the audit record carries `return_bps` but does not
compute PnL or scoring metrics. Use `--outcomes path` to attach outcome
attributions without supplying stored predictions.

```sh
cargo run -p asof-causality-cli -- generate --scenario late-heavy --events 100000 --symbols 1024 --late-rate 0.30 --feature-correction-rate 0.05 --seed 42 --out runs/late-heavy.pipe
```

Generates a deterministic pipe fixture with a fixed seed. The `late-heavy`
scenario intentionally shuffles physical file order; replay still sorts by
`(received_time, sequence, event_id)`. Every generated file includes a small
sentinel late-arrival sequence so the contrast checks have a known adversarial
case even when random rates are low.

```sh
cargo run -p asof-causality-cli -- run-suite --scenario late-heavy --events 100000 --symbols 1024 --seed 42 --out runs/late-heavy
```

Runs the start-to-finish path: generate an adversarial fixture, replay it, run
the checks, and write `events.pipe`, `predictions.pipe`, `checks.txt`, and
`summary.md`.

It also writes `manifest.json`, the run certificate for the output directory.
The manifest links the fixture hash, prediction-output hash, checks-output hash,
transcript hash, hash algorithm, invocation, UTC run timestamp, check counts,
Rust toolchain, and optional Git commit. A reviewer can compare the manifest and
artifacts to verify that a run's predictions, checks, and reported transcript
belong to the same execution. The machine-readable manifest contract lives in
[docs/manifest.schema.json](docs/manifest.schema.json).

Generated `runs/` artifacts are intentionally ignored by git so stale local
snapshots do not become part of the source artifact. Use
`make run-suite-late-heavy` to rebuild the canonical local `runs/late-heavy`
directory.

```sh
cargo run -p asof-causality-cli -- negative-control examples/lookahead-negative-control.pipe
```

Runs a negative-control fixture through the correct received-time replay and a
deliberately broken observed-time baseline. The fixture contains seed features,
a late positive feature, and a late negative feature correction; a backtester
ordered by observed time leaks both late records into earlier predictions.

```sh
cargo run -p asof-causality-cli -- negative-control examples/lookahead-negative-control.pipe --signal windowed-feature-sentiment
```

The windowed signal makes multi-input provenance visible. The expected
demonstration is:

```text
95:4:p_before_same_time_sequence|XYZ|0|n_seed_negative,n_seed_positive,n_seed_negative_2,n_same_time_later|95:5:n_same_time_later
120:6:p_before_late_feature|XYZ|1|n_seed_negative,n_seed_positive,n_seed_negative_2,n_same_time_later,n_late_positive|150:7:n_late_positive
170:10:p_before_correction|XYZ|1|n_seed_positive,n_seed_negative_2,n_same_time_later,n_late_positive,c_late_negative|180:9:c_late_negative
```

The leaky baseline is intentionally included as a negative control; it shows the
class of impossible prediction that the normal engine prevents.

```sh
cargo run -p asof-causality-cli -- negative-control examples/zscore-lookahead.pipe --signal windowed-zscore
```

The numeric fixture exercises the same boundary with continuous inputs. In the
broken observed-time baseline, `p_before_late_score` can see `px_late_spike`
before it was received. The received-time engine cannot, and the audit invariant
marks the spike as a future input.

```sh
cargo run -p asof-causality-cli -- negative-control examples/alfred-dgs10-sp500.pipe --signal windowed-zscore
```

This is the lookahead-bias falsification harness for the repo's real-data
path. The fixture uses ALFRED DGS10 Treasury-rate vintages as daily features
and FRED SP500 closes as next-day outcomes. It demonstrates that a daily SP500
prediction cannot use a same-day DGS10 observation until that row appears in the
next ALFRED vintage. See
[docs/real-data-demo.md](docs/real-data-demo.md) for source URLs and mapping.

```sh
cargo run -p asof-causality-cli -- sensitivity examples/alfred-dgs10-sp500.pipe --signal windowed-zscore --scenario lookahead --lookahead-range 0..100 --steps 4 --details --out runs/alfred-sensitivity
```

Runs a stability sweep outside the strict kernel: the baseline uses received
time, and the lookahead range removes 0% through 100% of each affected
feature's own `(received_time - observed_time)` lag. The command writes
`summary.jsonl`, a primary `sensitivity-curve.svg` with baseline x=0 and
sampled lookahead percentages on the x-axis, secondary static SVG charts
(`flip-rate.svg` and `input-change.svg`), optional `details.jsonl`, and
`manifest.json` with policy descriptors and transcript hashes.

Late-arrival attribution is a separate sensitivity scenario:

```sh
cargo run -p asof-causality-cli -- sensitivity examples/lookahead-negative-control.pipe --signal windowed-feature-sentiment --scenario late-arrivals --out runs/late-arrival-sensitivity
```

This builds automatic fixture-native lag buckets from late feature arrivals and
fully moves one bucket at a time to observed time. The output adds
`late-arrival-impact.svg`, which shows which lateness band accounts for changed
predictions. V1 still accepts raw `--shift-features` integer offsets as an
expert mode, but typed durations such as `-1d` are deferred until timestamp
semantics are first-class. Sensitivity descriptors and the manifest record
`calendar_aware: false`; on `YYYYMMDDHHMM` fixtures like ALFRED, intermediate
lookahead percentages are synthetic ordered-integer stresses, not calendar
durations.

The PAYEMS fixture anchors an actual ALFRED correction:

```sh
cargo run -p asof-causality-cli -- check examples/alfred-payems-revision.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- negative-control examples/alfred-payems-revision.pipe --signal windowed-zscore
```

It compares the PAYEMS `2019-01-01` observation across two ALFRED vintages:
`150587` in the `2020-02-01` vintage and `150134` in the `2020-03-01` vintage.
The second row is encoded as `feature_correction`, so strict received-time
replay must not let `p_after_initial_before_revision` use the revised value.

```sh
cargo run -p asof-causality-cli -- bench --events 1000000 --symbols 1024
```

Generates synthetic events and reports replay throughput for two state
representations: string-keyed map state and interned symbol-ID vector state.

## Why It Is Non-Trivial

The project does not try to find alpha. It builds the infrastructure required
before alpha research can be trusted:

- predictions are immutable audit records
- every prediction records the event IDs it used
- multi-input signals record up to eight input event keys inline by design
- prediction provenance is stored as compact inline event keys and rendered to
  human-readable IDs outside the replay path
- `max_input_replay_key <= prediction_replay_key` is checked
- outcomes are computed after predictions and cannot affect emitted predictions
- prefix-equivalence and future-mutation tests make leakage falsifiable
- replay is deterministic even when the physical input file is shuffled

See [docs/architecture.md](docs/architecture.md) and
[docs/measurements.md](docs/measurements.md) for the implementation shape and
measurement notes. See [docs/roadmap.md](docs/roadmap.md) for the planned
strategy layer, recipe-hash extension, and Parquet adapter.

## Compared To Backtesters

Many backtesting tools focus on simulating a portfolio over economic time.
asof-causality focuses on a narrower contract: whether a signal could have
known every input it used at prediction time. The negative-control fixture is
shipped with the repo so that the leak class is falsifiable, not just described.

## Run Certificates

`run-suite` writes a `manifest.json` beside the generated fixture, predictions,
checks, and summary. The manifest is a compact linkage proof for the run: it
records the invocation, run timestamp, source commit context, workspace dirty
flag, Rust toolchain, fixture hash, prediction-output hash, checks-output hash,
and final transcript hash. Public artifact hashes in the manifest use BLAKE3,
and the JSON contract is documented in
[docs/manifest.schema.json](docs/manifest.schema.json).

That means the result is not just "the CLI printed PASS." The output directory
contains enough identity to answer: which data, which signal, which executable
context, which checks, and which transcript produced this result?

## Testing And Mutation Checks

The normal gate is:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

The test suite includes fixture regressions, proptest-generated event streams,
CLI integration checks, JSON Schema validation for audit, run-suite manifest,
and sensitivity output, and snapshots for stable user-visible command output.

For mutation testing, install `cargo-mutants` and run this manually before
claiming the causality kernel is hardened:

```sh
cargo mutants \
  -f crates/asof-causality-core/src/checks.rs \
  -f crates/asof-causality-core/src/replay.rs \
  -f crates/asof-causality-core/src/log.rs
```

This is intentionally not a PR gate; it is a heavier review tool for checking
whether causality-critical tests kill behavioral mutations.

## Why Rust Here

The causality boundary depends on hiding future state from signal code and
making replay deterministic. Rust is useful for that narrow job: the core API
keeps mutable replay state crate-private, exposes only an opaque `AsOfView`, and
stores hot-path provenance in fixed-size values rather than heap-heavy records.
The repo keeps the interface small enough that a Python or AI-assisted research
pipeline could call this as an external verifier without moving all research
logic into Rust.

## AI-Ready Boundary

An LLM-backed signal would need the same shape as the built-ins: opaque
`AsOfView` in and `SymbolSnapshot` out. This repo does not ship an AI signal,
but the causality boundary is the right one for AI-assisted signals too: the
model cannot leak from data that never enters the view.

## Scope Of Conclusions

Synthetic fixtures prove point-in-time replay semantics and make leakage tests
repeatable. They do not prove market predictiveness, production data quality, or
exchange-grade latency. The benchmark is a single-node throughput measurement,
not a claim of HFT suitability.
