# asof-causality

asof-causality validates whether a historical signal used only data that was actually available when the prediction was made.

Most backtests ask, "Did this signal make money?" This asks the prior question: "Could this signal have known the inputs it used at that replay key?" It catches temporal leakage before a strategy backtest turns leaked signals into fake Sharpe.

It is not a backtester. It is a point-in-time signal verifier that sits upstream of strategy simulation, portfolio construction, fills, and PnL.

## 30-Second Demo

```sh
cargo run -p asof-cli -- negative-control examples/lookahead-negative-control.pipe --signal windowed-feature-sentiment
```

```text
ENGINE A: received-time replay (correct)
  impossible           0
  VERDICT              PASS

...

ENGINE B: observed-time replay (deliberately broken baseline)
  impossible           3
  VERDICT              FAIL
```

The broken engine sorts by `observed_time`, so late data leaks into earlier predictions. The correct engine sorts by `(received_time, received_sequence_number, event_id)` and emits zero impossible `PredictionRecord`s.

When installed, the binary is `asof`; during development, use `cargo run -p asof-cli --` as the prefix.

## Run It On Your Signal

For a built-in signal:

```sh
cargo run -p asof-cli -- check examples/alfred-dgs10-sp500.pipe --signal windowed-zscore
```

Then run `replay`, `audit`, and `sensitivity` for the same `--signal`.

For your own signal, write it in Rust:

1. Implement [`asof_causality::Signal`](crates/asof-causality/src/signal.rs) using the opaque `AsOfView`.
2. Register it in [`asof-signals`](crates/asof-signals/src/lib.rs) with a stable name and config descriptor.
3. Run the CLI with `--signal your-signal-name`.

The Rust boundary is intentional: the signal can inspect only the as-of view, not the full event stream. Python strategy and backtest code consume audited artifacts downstream.

Built-ins are validation fixtures, not alpha claims: `last-feature-sentiment`, `windowed-feature-sentiment`, `windowed-zscore`, and `vol-adjusted-momentum`.

## What A Green Run Proves

A green run proves that each emitted `PredictionRecord` is causal with respect to the event history provided: no recorded input arrived after the prediction's replay key, and adversarial checks did not find prefix, future-mutation, late-arrival, on-time-vs-late contrast, correction, outcome, replay, or audit-invariant failures.

It does not prove alpha, data authenticity, survivorship correctness, fill realism, portfolio correctness, or PnL.

## Why This Is Credible

- Signals receive an opaque `AsOfView`, not the full event stream.
- Short-window provenance is fixed-size and allocation-free in replay.
- Checks rerun prefixes, mutate future rows, and use negative controls.
- Audit JSONL and manifests bind signal identity, inputs, hashes, and tool context.

## Where Next

- [docs/demo.md](docs/demo.md): quant-facing workflow
- [docs/cli.md](docs/cli.md): commands
- [docs/audit-artifacts.md](docs/audit-artifacts.md): JSONL, manifests, schemas
- [docs/success-criteria.md](docs/success-criteria.md): check list and pass criteria
- [docs/architecture.md](docs/architecture.md): bitemporal kernel design
- [docs/roadmap.md](docs/roadmap.md): limitations and production path
- [docs/measurements.md](docs/measurements.md): benchmark methodology
