# Real-Data Demo: Daily ALFRED Rates With SP500 Outcomes

This fixture uses public daily Treasury-rate vintages from ALFRED and daily
SP500 closes from FRED. It is a causality demo, not an alpha claim: the question
is whether a daily SP500 prediction used only DGS10 observations available in
the ALFRED vintage stream at prediction time.

## Data Sources

- ALFRED DGS10 vintages:
  `https://alfred.stlouisfed.org/graph/alfredgraph.csv?id=DGS10&cosd=2020-03-10&coed=2020-03-20&vintage_date=YYYY-MM-DD&revision_date=YYYY-MM-DD`
  with vintage dates `2020-03-12`, `2020-03-13`, `2020-03-16`,
  `2020-03-17`, `2020-03-18`, `2020-03-19`, and `2020-03-20`.
- FRED SP500 closes:
  `https://fred.stlouisfed.org/graph/fredgraph.csv?id=SP500&cosd=2020-03-16&coed=2020-03-20`

The checked-in fixture is
[`examples/alfred-dgs10-sp500.pipe`](../examples/alfred-dgs10-sp500.pipe).
It can be regenerated from the public source CSVs with:

```sh
make verify-real-data-demo
```

## Mapping

- `observed_time`: DGS10 observation date at `15:00`, encoded as
  `YYYYMMDDHHMM`.
- `received_time`: ALFRED vintage date at `09:00`, encoded as `YYYYMMDDHHMM`.
- `score`: daily DGS10 change in percentage points.
- `prediction`: SP500 daily risk-on/risk-off prediction at `16:00`.
- `outcome`: next trading-day SP500 return in basis points, attached after the
  next close.

The fixture deliberately places predictions before the next DGS10 vintage is
available. A replay ordered by `observed_time` leaks same-day DGS10 rows into
the SP500 close prediction. The received-time engine blocks those rows until
the next vintage.

## Regression Target

The fixture includes a specific lookahead-bias trap:
`p_20200318_close_before_vintage` is emitted at `202003181600`, but
`dgs10_20200318_v20200319` is not received until `202003190900`.

The received-time replay must not include that DGS10 vintage in the prediction's
input provenance. The observed-time negative control should include it and mark
the prediction as impossible. This anchors the demo in the common ALFRED vintage
failure mode: treating today's final macro data as if it existed in yesterday's
research environment.

## Commands

```sh
uv run --script scripts/rebuild-alfred-example.py --check
cargo run -p asof-causality-cli -- check examples/alfred-dgs10-sp500.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- replay examples/alfred-dgs10-sp500.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- negative-control examples/alfred-dgs10-sp500.pipe --signal windowed-zscore
cargo run -p asof-causality-cli -- audit examples/alfred-dgs10-sp500.pipe --signal windowed-zscore --outcomes examples/alfred-dgs10-sp500.pipe --out runs/alfred-dgs10-sp500-audit.jsonl
```
