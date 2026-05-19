# asof-replay

asof-replay is a point-in-time backtest engine for late-arriving data. It
prevents lookahead bias by processing events in received-time order and verifies
correctness with adversarial replay tests.

Most backtests ask, "Did the signal work?" asof-replay asks the prior question:
"Could the signal have known what it used at the time?" The engine enforces a
two-clock event model, restricts signals to as-of state, records immutable
prediction logs, and checks that future data cannot affect past predictions.

## What This Builds

- a Rust workspace with a reusable core crate and CLI
- a pipe-delimited event format with `observed_time` and `received_time`
- deterministic replay by `(received_time, sequence)`
- a restricted signal API that receives only an opaque `AsOfView`
- immutable `PredictionRecord` output with input-event provenance
- adversarial leakage checks for late arrivals, corrections, labels, and
  shuffled physical input
- property tests for the universal leakage invariants over random bitemporal
  event streams
- a synthetic throughput benchmark comparing string-keyed state with interned
  symbol IDs
- a leakage contract that is type-enforced for Rust signals; Python strategy
  integration would be channel-enforced by sending only as-of snapshots, not the
  full dataset

The built-in signal is intentionally simple: the last received per-symbol news
or correction sentiment maps to `-1`, `0`, or `+1`. The non-trivial part is the
correctness cage around the signal. See [docs/extensions.md](docs/extensions.md)
for the intended Python and API-growth path.

## Quick Start

Install Rust 1.78+ if needed, then:

```sh
cargo run -p asof-replay-cli -- replay examples/late-arrival.pipe
cargo run -p asof-replay-cli -- check examples/late-arrival.pipe
cargo run -p asof-replay-cli -- replay examples/lookahead-negative-control.pipe
cargo run -p asof-replay-cli -- compare-leaky examples/lookahead-negative-control.pipe
cargo run -p asof-replay-cli -- generate --scenario late-heavy --events 100000 --symbols 1024 --late-rate 0.30 --correction-rate 0.05 --seed 42 --out runs/late-heavy.pipe
cargo run -p asof-replay-cli -- run-suite --scenario late-heavy --events 100000 --symbols 1024 --seed 42 --out runs/late-heavy
cargo run -p asof-replay-cli -- bench --events 1000000 --symbols 1024
cargo test
```

This repository has no runtime network dependency and uses only synthetic local
fixtures. It does not require market data, an LLM key, CUDA, or a database.

## Event Format

```text
event_id|observed_time|received_time|sequence|kind|symbol|payload
```

Events are replayed by `(received_time, sequence)`, not by physical file order
or observed time. A late event may influence future predictions, but it cannot
mutate old predictions.

## Current Commands

```sh
cargo run -p asof-replay-cli -- replay examples/late-arrival.pipe
```

Prints deterministic prediction records and a transcript hash.

```sh
cargo run -p asof-replay-cli -- check examples/late-arrival.pipe
```

Runs the adversarial replay suite against the fixture.

For large generated files, `check` samples 32 replay-key cutoffs by default
for the expensive prefix-equivalence and future-mutation checks. Use
`--exhaustive` for the full adversarial sweep on small fixtures, or
`--max-cutoffs N` to set the deterministic cutoff sample size.

```sh
cargo run -p asof-replay-cli -- generate --scenario late-heavy --events 100000 --symbols 1024 --late-rate 0.30 --correction-rate 0.05 --seed 42 --out runs/late-heavy.pipe
```

Generates a deterministic pipe fixture with a fixed seed. The `late-heavy`
scenario intentionally shuffles physical file order; replay still sorts by
`(received_time, sequence)`. Every generated file includes a small sentinel
late-arrival sequence so the contrast checks have a known adversarial case even
when random rates are low.

```sh
cargo run -p asof-replay-cli -- run-suite --scenario late-heavy --events 100000 --symbols 1024 --seed 42 --out runs/late-heavy
```

Runs the start-to-finish path: generate an adversarial fixture, replay it, run
the checks, and write `events.pipe`, `predictions.pipe`, `checks.txt`, and
`summary.md`.

```sh
cargo run -p asof-replay-cli -- replay examples/lookahead-negative-control.pipe
```

Replays a negative-control fixture: a late positive news event and a late
negative correction are observed before some predictions but received after
them. The correct output keeps the earlier predictions on the older state; a
backtester ordered by observed time would leak here.

```sh
cargo run -p asof-replay-cli -- compare-leaky examples/lookahead-negative-control.pipe
```

Runs the same fixture through the correct received-time replay and a deliberately
broken observed-time baseline. The expected demonstration is:

```text
received-time replay: PASS
observed-time replay (leaky baseline): FAIL
```

The leaky baseline is intentionally included as a negative control; it shows the
class of impossible prediction that the normal engine prevents.

```sh
cargo run -p asof-replay-cli -- bench --events 1000000 --symbols 1024
```

Generates synthetic events and reports replay throughput for two state
representations: string-keyed map state and interned symbol-ID vector state.

## Why It Is Non-Trivial

The project does not try to find alpha. It builds the infrastructure required
before alpha research can be trusted:

- predictions are immutable audit records
- every prediction records the event IDs it used
- prediction provenance is stored as compact inline event keys and rendered to
  human-readable IDs outside the replay path
- `max_input_replay_key <= prediction_replay_key` is checked, where a replay key
  is `(received_time, sequence)`
- labels are computed after predictions and cannot affect emitted predictions
- prefix-equivalence and future-mutation tests make leakage falsifiable
- seven universal leakage invariants are property-tested over randomly generated
  bitemporal streams; the eighth check, `on_time_vs_late_contrast`, is a
  positive control exercised against curated and generated adversarial streams,
  with generator coverage tests requiring multiple late-contrast opportunities
- replay is deterministic even when the physical input file is shuffled

See [docs/architecture.md](docs/architecture.md),
[docs/threat-model.md](docs/threat-model.md), and
[docs/measurements.md](docs/measurements.md) for the implementation shape,
boundary limits, and measurement notes.

## Scope Of Conclusions

Synthetic fixtures prove point-in-time replay semantics and make leakage tests
repeatable. They do not prove market predictiveness, production data quality, or
exchange-grade latency. The benchmark is a single-node throughput measurement,
not a claim of HFT suitability.
