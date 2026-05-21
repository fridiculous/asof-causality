# Real-Data Demo: ALFRED Vintages And Revisions

The primary fixture uses public daily Treasury-rate vintages from ALFRED and
daily SP500 closes from FRED. The revision fixtures use ALFRED PAYEMS vintages.
These are causality demos, not alpha claims: the question is whether each
replay-derived `PredictionRecord` used only macro observations available in the
ALFRED vintage stream at prediction time.

## Data Sources

- ALFRED DGS10 vintages:
  `https://alfred.stlouisfed.org/graph/alfredgraph.csv?id=DGS10&cosd=2020-03-10&coed=2020-03-20&vintage_date=YYYY-MM-DD&revision_date=YYYY-MM-DD`
  with vintage dates `2020-03-12`, `2020-03-13`, `2020-03-16`,
  `2020-03-17`, `2020-03-18`, `2020-03-19`, and `2020-03-20`.
- FRED SP500 closes:
  `https://fred.stlouisfed.org/graph/fredgraph.csv?id=SP500&cosd=2020-03-16&coed=2020-03-20`
- ALFRED PAYEMS minimal revision fixture:
  `https://alfred.stlouisfed.org/graph/alfredgraph.csv?id=PAYEMS&cosd=2019-01-01&coed=2019-03-01&vintage_date=YYYY-MM-DD&revision_date=YYYY-MM-DD`
  with vintage dates `2020-02-01` and `2020-03-01`.
- ALFRED PAYEMS larger revision fixture:
  `https://alfred.stlouisfed.org/graph/alfredgraph.csv?id=PAYEMS&cosd=2019-01-01&coed=2021-12-01&vintage_date=YYYY-MM-DD&revision_date=YYYY-MM-DD`
  with monthly vintage dates from `2020-02-01` through `2021-12-01`.

The checked-in fixture is
[`examples/alfred-dgs10-sp500.pipe`](../examples/alfred-dgs10-sp500.pipe).
It can be regenerated from the public source CSVs with:

```sh
make verify-real-data-demo
```

The checked-in PAYEMS revision fixtures are
[`examples/alfred-payems-revision.pipe`](../examples/alfred-payems-revision.pipe)
and
[`examples/alfred-payems-revisions-2020.pipe`](../examples/alfred-payems-revisions-2020.pipe).
They can be regenerated with:

```sh
make verify-real-revision-demo
```

Verification requires internet access to the public ALFRED and FRED CSV
endpoints.

## Mapping

- `observed_time`: DGS10 observation date at `15:00`, encoded as
  `YYYYMMDDHHMM`.
- `received_time`: ALFRED vintage date at `09:00`, encoded as `YYYYMMDDHHMM`.
- `score`: daily DGS10 change in percentage points, declared as
  `FeatureDType::FixedDecimal { scale: 6 }`.
- `prediction`: scheduled SP500 close event at `16:00`; replay evaluates the
  signal and appends a `PredictionRecord`.
- `outcome`: next trading-day SP500 return in basis points, attached after the
  next close.

The DGS10 feature rows use ALFRED vintages. SP500 outcome rows use FRED closes;
their `received_time` is a fixture convention for post-close attribution, not a
vintage claim.

The fixture deliberately places prediction events before the next DGS10 vintage
is available. A replay ordered by `observed_time` leaks same-day DGS10 rows into
the SP500 close evaluation. The received-time engine blocks those rows until the
next vintage.

## Regression Target

The fixture includes a specific lookahead-bias trap:
`p_20200318_close_before_vintage` is emitted at `202003181600`, but
`dgs10_20200318_v20200319` is not received until `202003190900`.

The received-time replay must not include that DGS10 vintage in the resulting
`SignalEvaluation` input provenance. The observed-time negative control should
include it and mark the `PredictionRecord` as impossible. This anchors the demo
in the common ALFRED vintage failure mode: treating today's final macro data as
if it existed in yesterday's research environment.

## Real Revision Target

The PAYEMS fixture includes a real ALFRED correction:

- observation date: `2019-01-01`
- value in the `2020-02-01` vintage: `150587`
- value in the `2020-03-01` vintage: `150134`

The checked-in fixture encodes the first value as a `feature` and the second
value as a `feature_correction`. The prediction
`p_after_initial_before_revision` is emitted on `2020-02-14`, after the first
vintage but before the revised vintage arrives on `2020-03-01`. Received-time
replay must not use the correction for that prediction. An observed-time replay
does use it and should mark the prediction as impossible.

The larger PAYEMS fixture is intended for sensitivity runs rather than
hand-inspection. It spans ALFRED monthly vintages from `2020-02-01` through
`2021-12-01` and emits:

- first-seen PAYEMS observations as `feature`
- revised PAYEMS observations as `feature_correction`
- one mid-month prediction after each vintage

That gives 133 actual events: 34 features, 76 feature corrections, and 23
predictions. A strict received-time check passes; the observed-time negative
control surfaces many impossible predictions because it can see future
corrections by observation date. The late-arrival sensitivity output uses a
cumulative exposure curve: each sampled point moves all late feature and
correction events by that percentage of their own observed-to-received lag and
reports cumulative unique new input admissions as the y-axis. The chart's
visible y-axis includes absolute admission counts; point tooltips summarize the
effective lag removed as min/median/p90/max calendar durations when timestamps
parse as `YYYYMMDDHHMM`. The replay transform is still percent-of-each-event-lag,
not a calendar-duration subtraction; calendar durations are reported only to make
the sampled lag removal easier to interpret.

## Commands

```sh
uv run --script scripts/rebuild-alfred-example.py --check
uv run --script scripts/rebuild-alfred-revision-example.py --check
uv run --script scripts/rebuild-alfred-revision-example.py --variant large --check
cargo run -p asof-causality-cli -- check examples/alfred-dgs10-sp500.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- check examples/alfred-payems-revision.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- check examples/alfred-payems-revisions-2020.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- replay examples/alfred-dgs10-sp500.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- negative-control examples/alfred-dgs10-sp500.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- negative-control examples/alfred-payems-revision.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- negative-control examples/alfred-payems-revisions-2020.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- sensitivity examples/alfred-payems-revisions-2020.pipe --signal windowed-zscore --scenario late-arrivals --out runs/alfred-payems-large-sensitivity
cargo run -p asof-causality-cli -- audit examples/alfred-dgs10-sp500.pipe --signal windowed-zscore --outcomes examples/alfred-dgs10-sp500.pipe --out runs/alfred-dgs10-sp500-audit.jsonl
cargo run -p asof-causality-cli -- check examples/alfred-dgs10-sp500.pipe --signal vol-adjusted-momentum
cargo run -p asof-causality-cli -- negative-control examples/alfred-dgs10-sp500.pipe --signal vol-adjusted-momentum
```
