# asof-causality

asof-causality is a deterministic causality test suite for lookahead bias. It
checks whether a historical signal only used data that was knowable at
prediction time.

Most backtests ask, "Did the signal work?" asof-causality asks the prior
question: "Could the signal have known what it used at the time?" The engine
enforces a two-clock event model, restricts signals to as-of state, records
immutable prediction logs, and checks that future data cannot affect past
predictions.

## What This Builds

- a Rust workspace with a reusable core crate and CLI
- a pipe-delimited event format with `observed_time` and `received_time`
- canonical event roles: `feature`, `feature_correction`, `prediction`, and
  `outcome`
- deterministic replay by `(received_time, sequence, event_id)`
- a restricted signal API that receives only an opaque `AsOfView`
- built-in single-input and windowed multi-input signals
- immutable `PredictionRecord` output with input-event provenance
- interned symbol IDs in replay state and prediction records, rendered back to
  human symbols in transcripts
- adversarial leakage checks for late arrivals, feature corrections, outcomes, and
  shuffled physical input
- a synthetic throughput benchmark comparing string-keyed state with interned
  symbol IDs

The built-in signals are intentionally simple: one reads the last received
per-symbol feature sentiment, and one reads a bounded recent feature window. The
non-trivial part is the correctness cage around the signal.

## Quick Start

Install Rust 1.78+ if needed, then:

```sh
cargo run -p asof-causality-cli -- replay examples/late-arrival.pipe
cargo run -p asof-causality-cli -- check examples/late-arrival.pipe
cargo run -p asof-causality-cli -- negative-control examples/lookahead-negative-control.pipe
cargo run -p asof-causality-cli -- negative-control examples/lookahead-negative-control.pipe --signal windowed-feature-sentiment
cargo run -p asof-causality-cli -- generate --scenario late-heavy --events 100000 --symbols 1024 --late-rate 0.30 --feature-correction-rate 0.05 --seed 42 --out runs/late-heavy.pipe
cargo run -p asof-causality-cli -- run-suite --scenario late-heavy --events 100000 --symbols 1024 --seed 42 --out runs/late-heavy
cargo run -p asof-causality-cli -- bench --events 1000000 --symbols 1024
cargo test
```

This repository has no runtime network dependency and uses only synthetic local
fixtures. It does not require market data, an LLM key, CUDA, or a database.

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
bounded multi-input signal. The default is `last-feature-sentiment`.

```sh
cargo run -p asof-causality-cli -- check examples/late-arrival.pipe
```

Runs the adversarial replay suite against the fixture.

For large generated files, `check` samples 32 received-time cutoffs by default
for the expensive prefix-equivalence and future-mutation checks. Use
`--exhaustive` for the full adversarial sweep on small fixtures, or
`--max-cutoffs N` to set the deterministic cutoff sample size.

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
`summary.md`. It also writes `manifest.json`, which links the fixture hash,
signal-version hash, checks hash, transcript hash, hash algorithm, invocation,
UTC run timestamp, check counts, and optional Git commit for the run.

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
ENGINE A: received-time replay (correct)
  VERDICT              PASS

ENGINE B: observed-time replay (deliberately broken baseline)
  impossible           3
  VERDICT              FAIL

LEAKED PREDICTIONS (engine B)
```

The leaky baseline is intentionally included as a negative control; it shows the
class of impossible prediction that the normal engine prevents.

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
measurement notes.

## Compared To Backtesters

Many backtesting tools focus on simulating a portfolio over economic time.
asof-causality focuses on a narrower contract: whether a signal could have
known every input it used at prediction time. The negative-control fixture is
shipped with the repo so that the leak class is falsifiable, not just described.

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
