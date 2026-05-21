# Demo: Signal Validation Workflow

This is the single demo flow for the repo. It shows where `asof-causality`
sits in a quant research system:

```text
signal implementation -> replay/check/audit/sensitivity -> signal analysis -> strategy backtest
```

The demo validates a signal. It does not backtest a strategy, choose positions,
or certify PnL.

## Scenario

A quant is developing a daily macro signal:

```text
ALFRED DGS10 Treasury-rate vintages -> SP500 risk-on/risk-off score
```

The signal idea is not the point of the demo. The point is to verify that each
SP500 prediction used only Treasury-rate observations that were available in
the ALFRED vintage stream at prediction time.

The main fixture is:

```text
examples/alfred-dgs10-sp500.pipe
```

It contains feature rows, prediction rows, and outcome rows for a small
end-to-end signal-validation run. The trap is deliberately curated from real
ALFRED/FRED vintages; it is not presented as a broad historical discovery.

## Workflow

1. The quant creates or modifies a Rust signal.
2. The platform replays that signal over a point-in-time dataset.
3. The platform runs causality check methods.
4. The platform writes audit and sensitivity artifacts.
5. The quant analyzes the certified signal stream.
6. Strategy backtests consume the artifacts downstream.

For this demo, the signal is the built-in `windowed-zscore` from
`asof-signals`.

## Commands

Run these one at a time. The first command is the main demo gate.

Run the causality gate:

```sh
cargo run -p asof-cli -- check examples/alfred-dgs10-sp500.pipe --signal windowed-zscore
```

Inspect replayed predictions if you want to see the emitted signal stream:

```sh
cargo run -p asof-cli -- replay examples/alfred-dgs10-sp500.pipe --signal windowed-zscore
```

Write audit JSONL for downstream analysis:

```sh
cargo run -p asof-cli -- audit examples/alfred-dgs10-sp500.pipe \
  --signal windowed-zscore \
  --outcomes examples/alfred-dgs10-sp500.pipe \
  --out runs/alfred-dgs10-sp500-audit.jsonl
```

Run sensitivity over the larger PAYEMS revision fixture:

```sh
cargo run -p asof-cli -- sensitivity examples/alfred-payems-revisions-2020.pipe \
  --signal windowed-zscore \
  --scenario late-arrivals \
  --out runs/alfred-payems-large-sensitivity
```

Run the negative control to see what a leaky observed-time replay would do:

```sh
cargo run -p asof-cli -- negative-control examples/alfred-dgs10-sp500.pipe --signal windowed-zscore
```

The sensitivity command is intentionally separate because it writes a directory
of analysis artifacts and uses the larger PAYEMS revision fixture.

When installed, replace `cargo run -p asof-cli --` with `asof`.

## Expected Result

The received-time engine should pass:

```text
CHECK METHODS ... PASS
```

The negative control should fail for the deliberately broken observed-time
baseline. The specific trap is:

```text
prediction: p_20200318_close_before_vintage at 202003181600
leaked row: dgs10_20200318_v20200319 received at 202003190900
```

A naive replay ordered by observation date can see the same-day DGS10 vintage at
the SP500 close. The received-time replay cannot, because the vintage was not
available until the next morning. The fixture is hand-constructed to make that
failure mode visible in a small demo.

That failure is a signal-construction problem, not a strategy problem. The
quant should fix the feature join, availability timestamp, or provenance
logging before analyzing predictive quality.

## Data Sources

- ALFRED DGS10 vintages:
  `https://alfred.stlouisfed.org/graph/alfredgraph.csv?id=DGS10&cosd=2020-03-10&coed=2020-03-20&vintage_date=YYYY-MM-DD&revision_date=YYYY-MM-DD`
  with vintage dates `2020-03-12`, `2020-03-13`, `2020-03-16`,
  `2020-03-17`, `2020-03-18`, `2020-03-19`, and `2020-03-20`.
- FRED SP500 closes:
  `https://fred.stlouisfed.org/graph/fredgraph.csv?id=SP500&cosd=2020-03-16&coed=2020-03-20`
- ALFRED PAYEMS revision fixtures:
  `https://alfred.stlouisfed.org/graph/alfredgraph.csv?id=PAYEMS&cosd=2019-01-01&coed=2021-12-01&vintage_date=YYYY-MM-DD&revision_date=YYYY-MM-DD`

Regenerate the checked-in fixtures:

```sh
make verify-real-data-demo
make verify-real-revision-demo
```

Run these targets one at a time. They require internet access to public
ALFRED/FRED CSV endpoints, but they do not require an API key.

## Mapping

- `observed_time`: economic observation timestamp.
- `received_time`: when the vintage or correction became available to the
  replay engine.
- `score`: daily DGS10 or PAYEMS change encoded as fixed-point numeric payload.
- `prediction`: scheduled signal evaluation point.
- `outcome`: later return attribution, attached after replay and excluded from
  signal state.

The DGS10 fixture uses ALFRED vintages for feature availability and FRED closes
for SP500 outcomes. The PAYEMS fixtures encode real ALFRED revisions as
`feature_correction` rows.

## What This Proves

A green run proves:

```text
Every replay-derived prediction used only inputs available at its replay key.
```

It does not prove:

```text
The signal is predictive.
The strategy is profitable.
The backtest has realistic fills.
The universe construction is correct.
The input timestamps are authentic.
```

`asof-causality` is one signal-validity gate. Predictive analysis and strategy
backtesting happen downstream.
