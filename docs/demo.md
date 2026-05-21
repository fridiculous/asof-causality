# Signal Validation Demo

This demo frames `asof-causality` as one step in validating a quant signal. It
does not backtest a strategy, choose positions, or certify PnL. It checks the
first thing a researcher needs to know before trusting any signal analytics:

```text
Was each signal value computed only from data available at that time?
```

The Python script is a thin platform-layer wrapper, not a Python binding. It
resolves a named dataset, invokes the Rust CLI as a subprocess, reads the audit
JSONL, and prints the signal-validation summary a quant would expect during
research.

## The Signal Story

A quant wants to test a daily macro signal:

```text
DGS10 Treasury-rate vintage history -> SP500 risk-on/risk-off score
```

The signal idea is simple: use recent DGS10 changes from ALFRED vintages to emit
a daily SP500 score. Before asking whether that score predicts returns, the
quant needs to verify that the score does not use DGS10 rows that were not yet
available.

The demo uses:

```text
examples/alfred-dgs10-sp500.pipe
```

That fixture includes the data, prediction points, and outcomes for a tiny
signal-validation run.

## Where This Fits

A complete signal research loop is:

```text
1. Define signal idea and feature inputs
2. Generate signal values over a universe and time window
3. Run `asof` as the temporal-validity gate
4. Attach outcomes after the fact
5. Analyze predictive quality: IC, hit rate, decay, coverage, stability
6. If useful, pass the signal to a strategy/backtester
```

This demo covers step 3 and shows how outcomes can be attached for later
analysis. It does not do steps 5 or 6.

## Envisioned Systems Workflow

The workflow is signal-first:

```text
1. Quant creates or modifies a Rust signal.
2. Platform runs replay, check, audit, and sensitivity for that signal.
3. Platform stores the resulting artifacts.
4. Quants analyze the signal artifacts directly.
5. Strategies and backtests consume the certified prediction stream.
6. If the signal fails or looks weak, the quant iterates.
```

The platform command sequence is:

```sh
asof replay DATASET --signal SIGNAL --out runs/SIGNAL/predictions.pipe
asof check DATASET --signal SIGNAL --out runs/SIGNAL/checks.txt
asof audit DATASET --signal SIGNAL --out runs/SIGNAL/audit.jsonl
asof sensitivity DATASET --signal SIGNAL --out runs/SIGNAL/sensitivity.jsonl
```

The important boundary is simple: `asof` validates and packages signal outputs.
Analysis notebooks and strategy backtests consume those artifacts later. If a
run fails, the quant fixes the signal or its data assumptions and reruns the
same workflow.

## Happy Path

Run the signal-validation wrapper:

```sh
uv run --script scripts/quant_workflow_demo.py \
  --dataset macro-research-v1 \
  --signal windowed-zscore
```

The script resolves `macro-research-v1` through a tiny in-script dataset
registry, then runs:

```sh
cargo run -p asof-cli -- audit \
  examples/alfred-dgs10-sp500.pipe \
  --signal windowed-zscore \
  --out runs/demo/audit.jsonl \
  --outcomes examples/alfred-dgs10-sp500.pipe
```

Expected shape:

```text
Signal causality check: PASS
  Predictions audited       4
  Non-causal                0
  Outcomes attached         4
  Stored predictions matched not supplied

Artifacts
  audit JSONL               runs/demo/audit.jsonl
  manifest                  runs/demo/manifest.json
```

The result means the signal stream passed the as-of causality gate. The quant
can now move to predictive analysis: forward returns, rank IC, decay, coverage,
and robustness checks.

## Failure Path

The fixture contains a known lookahead trap. The prediction
`p_20200318_close_before_vintage` is emitted at `202003181600`, while the
same-day DGS10 vintage `dgs10_20200318_v20200319` is not received until
`202003190900`. A replay ordered by observed time leaks that macro row into the
close prediction. A replay ordered by received time does not.

To show the failure mode:

```sh
uv run --script scripts/quant_workflow_demo.py \
  --dataset macro-research-v1 \
  --signal windowed-zscore \
  --simulate-leak
```

The script runs the normal audit and then invokes the existing
`negative-control` command. The negative control compares the correct
received-time replay with the deliberately broken observed-time baseline. The
wrapper parses the impossible predictions and renders them as signal-debugging
examples:

```text
Leak simulation: observed-time baseline
  Impossible predictions    N

Example 1
  Prediction
    event        p_20200318_close_before_vintage
    replay key   202003181600:8:p_20200318_close_before_vintage
    symbol       SP500
  Leaked input
    event        dgs10_20200318_v20200319
    observed     202003181500
    received     202003190900
  Problem
    input replay key > prediction replay key ...
    prediction used event that arrived later ...
  Likely fix
    Use received-time/as-of joins and preserve vendor or ingestion availability.
```

That failure is a signal-construction problem, not a strategy problem. The
researcher should fix the feature join, availability timestamp, or provenance
logging before analyzing the signal's predictive quality.

## Ownership

The quant owns:

- the signal idea
- feature definitions
- signal configuration
- prediction schedule
- interpretation of signal analytics

The platform or data team owns:

- point-in-time dataset construction
- received-time policy
- vendor and warehouse adapters
- feature provenance logging
- artifact storage and CI integration

`asof-causality` sits between those responsibilities. It validates that the
signal values produced for a point-in-time dataset are temporally valid.

## What The Platform Layer Is Doing

The script demonstrates the integration contract a real research platform would
hide behind a more polished API:

- Dataset registry: `macro-research-v1` resolves to the checked-in fixture.
- Signal selection: `--signal windowed-zscore` resolves through the
  `asof-signals` registry.
- Audit runner: the script invokes the Rust CLI as a subprocess.
- Artifact identity: the wrapper writes `audit.jsonl` and a small manifest with
  hashes.
- Human summary: the script turns audit JSONL into a concise signal-validation
  report.

A production integration would replace the in-script registry with a real
dataset registry, use Parquet/Arrow adapters, and require upstream prediction
loggers to emit the same feature-recipe hash contract as the kernel:

```text
signal name + config descriptor + ordered input event keys
```

`--allow-missing-recipe-hash` should be treated as a legacy import escape hatch,
not the standard path for certifying stored predictions.

## What This Does Not Certify

The green check means:

```text
The signal stream is causally valid with respect to the event history.
```

It does not mean:

```text
The signal is predictive.
The strategy is profitable.
The backtest has realistic fills.
The universe construction is correct.
The portfolio simulation is valid.
```

`asof-causality` is the temporal-validity gate for signal research. Predictive
analysis and strategy backtesting happen downstream.
