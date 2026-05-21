# asof-causality

asof-causality is a deterministic correctness cage for temporal leakage. It
tests whether a historical signal value used only data that was knowable at the
exact replay key when the prediction was made.

Most backtests ask, "Did the signal work?" This repository asks the prior
systems question: "Could the signal or training-data pipeline have known what it
used at that time?"

This is not a backtester. It sits upstream of strategy simulation and verifies
point-in-time signal correctness before anyone trusts predictive analytics,
portfolio logic, fills, or PnL.

## 30-Second Demo

```sh
cargo run -p asof-cli -- negative-control examples/lookahead-negative-control.pipe --signal windowed-feature-sentiment
```

The command runs the same fixture through two replay engines:

```text
ENGINE A: received-time replay (correct)
  impossible           0
  VERDICT              PASS

ENGINE B: observed-time replay (deliberately broken baseline)
  impossible           3
  VERDICT              FAIL
```

The broken engine sorts by `observed_time`, so it leaks late data into earlier
predictions. The correct engine sorts by
`(received_time, received_sequence_number, event_id)` and emits zero impossible
`PredictionRecord`s.

When installed, the binary is `asof`:

```sh
asof replay examples/late-arrival.pipe
asof check examples/late-arrival.pipe
asof audit examples/late-arrival.pipe
```

During development, use:

```sh
cargo run -p asof-cli -- replay examples/late-arrival.pipe
cargo run -p asof-cli -- check examples/late-arrival.pipe
cargo run -p asof-cli -- audit examples/late-arrival.pipe
```

See [docs/cli.md](docs/cli.md) for the full command reference.

## What This Builds

- `asof-causality`: signal-agnostic Rust replay kernel
- `asof-signals`: built-in signals and registry
- `asof-cli`: user-facing binary named `asof`
- deterministic replay over a two-clock event model:
  `observed_time` and `received_time`
- immutable `PredictionRecord`s with input-event provenance
- JSONL audit output and run manifests with schemas in
  [docs/schemas](docs/schemas)
- adversarial checks for prefix equivalence, future mutation, late arrivals,
  feature corrections, outcome separation, deterministic replay, and audit
  invariants

The built-in signals are test fixtures for the replay cage, not alpha claims:
`last-feature-sentiment`, `windowed-feature-sentiment`, `windowed-zscore`, and
`vol-adjusted-momentum`.

## Why It Is Non-Trivial

- **Compile-time caging:** signal code receives an opaque `AsOfView`, not the
  full event stream.
- **Zero-allocation provenance:** short-window input keys are carried in a
  fixed-capacity inline set rather than heap-allocating per prediction.
- **Dense replay state:** symbols are interned before replay and state is
  indexed by dense `SymbolSlot`s in the hot loop.
- **Adversarial falsification:** the checker reruns prefixes and mutates future
  rows to prove old predictions are stable.
- **Audit artifacts:** `audit.jsonl` and `manifest.json` bind predictions,
  checks, hashes, signal identity, invocation, and tool context.

The implementation details live in [docs/architecture.md](docs/architecture.md)
and the benchmark methodology lives in
[docs/measurements.md](docs/measurements.md).

## Quick Signal Workflow

```text
1. Quant creates or modifies a Rust signal.
2. Platform runs replay, check, audit, and sensitivity for that signal.
3. Platform stores the resulting artifacts.
4. Quants analyze the certified signal stream.
5. Strategies/backtests consume the artifacts downstream.
```

The runnable wrapper demonstrates this shape:

```sh
uv run --script scripts/quant_workflow_demo.py --dataset macro-research-v1 --signal windowed-zscore
```

See [docs/demo.md](docs/demo.md) for the quant workflow and
[docs/real-data-demo.md](docs/real-data-demo.md) for the ALFRED/FRED
case study showing macro lookahead bias.

## Known Limitations

- Inline provenance is capped at `MAX_INPUTS_PER_PREDICTION = 8`.
  Larger-window signals need recipe hashes or snapshot manifests.
- The v1 pipe CLI is a batch verifier: it reads events into memory and globally
  sorts them before replay. Out-of-core replay belongs to the Arrow/Parquet
  roadmap.
- CLI outcome attachment currently joins on `(symbol, prediction_replay_key)`.
  Production outcome APIs should join on economic target keys and resolve replay
  identity internally.
- `sentiment` payloads are legacy fixture domains. Production quant features
  should move toward typed numeric Arrow/Parquet columns.
- Large-input `check` runs sample 32 deterministic cutoffs by default because
  prefix-equivalence and future-mutation checks rerun replay for selected
  prefixes.
- The CLI trusts the event file's `received_time`. Production deployments need
  an infrastructural timestamp root of trust.

See [docs/roadmap.md](docs/roadmap.md) for the planned production path:
long-window provenance, Arrow/Parquet I/O, outcome joins, out-of-core replay,
and Python/IPC strategy handoff.

## Documentation Map

- [docs/architecture.md](docs/architecture.md): bitemporal model, ingestion
  boundary, root of trust, safety-lag reasoning
- [docs/cli.md](docs/cli.md): command reference
- [docs/audit-artifacts.md](docs/audit-artifacts.md): audit JSONL, manifests,
  hashes, and schema contracts
- [docs/demo.md](docs/demo.md): quant-facing signal validation workflow
- [docs/real-data-demo.md](docs/real-data-demo.md): ALFRED/FRED and PAYEMS
  fixtures
- [docs/measurements.md](docs/measurements.md): benchmark methodology and
  throughput notes
- [docs/problem.md](docs/problem.md): domain problem and relation to
  backtesters
- [docs/roadmap.md](docs/roadmap.md): production platform evolution
- [docs/success-criteria.md](docs/success-criteria.md): verification criteria

## Verification

```sh
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Mutation testing is intentionally a heavier review tool:

```sh
cargo mutants \
  -f crates/asof-causality/src/checks.rs \
  -f crates/asof-causality/src/replay.rs \
  -f crates/asof-causality/src/log.rs
```

## Scope Of Conclusions

A green audit means the signal stream is causally valid with respect to the
event history it was given. It does not prove alpha, data authenticity, market
predictiveness, fill realism, or portfolio correctness.
