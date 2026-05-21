# CLI Reference

The installed binary is `asof`. During development, prefix commands with
`cargo run -p asof-cli --`.

Run examples one at a time. Multi-line examples with trailing backslashes are a
single command; stacked one-line examples are alternatives.

```sh
asof replay examples/late-arrival.pipe
cargo run -p asof-cli -- replay examples/late-arrival.pipe
```

## Signals

The CLI resolves built-ins through `asof-signals`:

- `last-feature-sentiment` (default)
- `windowed-feature-sentiment`
- `windowed-zscore`
- `vol-adjusted-momentum`

Use `--signal name` on commands that evaluate a signal: `replay`, `check`,
`audit`, `negative-control`, `run-suite`, and `sensitivity`.

```sh
asof check examples/alfred-dgs10-sp500.pipe --signal windowed-zscore
```

## replay

```sh
asof replay examples/late-arrival.pipe
```

Prints deterministic `PredictionRecord`s and a transcript hash.

```text
prediction_replay_key|symbol|signal_value|input_event_ids|max_input_replay_key
580:3:p1|AAPL|0|-|-
590:4:p2|AAPL|1|n1|585:2:n1
transcript_hash=...
outcomes_seen=1
```

## check

```sh
asof check examples/late-arrival.pipe
asof check examples/late-arrival.pipe --exhaustive
asof check examples/late-arrival.pipe --max-cutoffs 64
```

These are alternative check invocations. They run adversarial checks against the
fixture. Large inputs sample deterministic
received-time cutoffs by default; `--exhaustive` is intended for small fixtures.

## audit

```sh
asof audit examples/late-arrival.pipe --signal windowed-feature-sentiment
asof audit examples/late-arrival.pipe --out runs/audit.jsonl
```

These are alternative audit invocations. Without `--out`, audit JSONL is printed
to stdout. With `--out`, it is written to the given path. The schema is
[schemas/audit.schema.json](../schemas/audit.schema.json).

Stored prediction comparison:

```sh
asof audit events.pipe stored_predictions.jsonl outcomes.pipe --out audit.jsonl
```

Stored predictions are matched by `(symbol, prediction_replay_key)` and should
include `signal_value` plus `feature_recipe_hash`. Use
`--allow-missing-recipe-hash` only for legacy stored predictions that can be
matched on `signal_value` alone.

Outcome attribution without stored predictions:

```sh
asof audit events.pipe --outcomes outcomes.pipe --out audit.jsonl
```

The current CLI outcome adapter expects `prediction_replay_key` and
`return_bps`. Production outcome joins should use economic target keys and
resolve replay identity inside the platform.

## negative-control

```sh
asof negative-control examples/lookahead-negative-control.pipe
asof negative-control examples/lookahead-negative-control.pipe --signal windowed-feature-sentiment
asof negative-control examples/zscore-lookahead.pipe --signal windowed-zscore
asof negative-control examples/zscore-lookahead.pipe --signal vol-adjusted-momentum
```

These are alternative negative-control invocations. Each runs one fixture
through the correct received-time engine and a deliberately broken observed-time
baseline. The observed-time baseline is not a production mode; it exists to show
the impossible records a naive replay would emit.

## generate

```sh
asof generate \
  --scenario late-heavy \
  --events 100000 \
  --symbols 1024 \
  --late-rate 0.30 \
  --feature-correction-rate 0.05 \
  --outcome-rate 0.10 \
  --seed 42 \
  --out runs/late-heavy.pipe
```

Generates deterministic adversarial pipe fixtures. The `late-heavy` scenario
shuffles physical file order; replay still sorts by
`(received_time, received_sequence_number, event_id)`. Use `--outcome-rate`
to control generated outcome rows; `--label-rate` is accepted as a legacy alias.

## run-suite

```sh
asof run-suite \
  --scenario late-heavy \
  --events 100000 \
  --symbols 1024 \
  --outcome-rate 0.10 \
  --seed 42 \
  --out runs/late-heavy
```

Runs the start-to-finish artifact path:

```text
events.pipe -> predictions.pipe -> checks.txt -> summary.md -> manifest.json
```

Generated `runs/` artifacts are ignored by git. Use
`make run-suite-late-heavy` to rebuild the canonical local run.

## sensitivity

Lookahead stress:

```sh
asof sensitivity examples/alfred-dgs10-sp500.pipe \
  --signal windowed-zscore \
  --scenario lookahead \
  --lookahead-range 0..100 \
  --steps 4 \
  --details \
  --out runs/alfred-sensitivity
```

Late-arrival attribution:

```sh
asof sensitivity examples/lookahead-negative-control.pipe \
  --signal windowed-feature-sentiment \
  --scenario late-arrivals \
  --out runs/late-arrival-sensitivity
```

Outputs include `summary.jsonl`, SVG charts, optional `details.jsonl`, and
`manifest.json`. In the `late-arrivals` scenario, `flip_rate` is not expected
to be monotonic across cumulative policies: moving more late inputs earlier can
change which prior inputs are visible at each prediction cutoff. Sensitivity
schemas live in [schemas](../schemas).

## bench

```sh
asof bench --events 1000000 --symbols 1024
```

Generates synthetic events and reports replay throughput for string-keyed state
versus dense symbol-slot state. Methodology and caveats are in
[measurements.md](measurements.md).

## Real-Data Helpers

```sh
uv run --script scripts/rebuild-alfred-example.py --check
uv run --script scripts/rebuild-alfred-revision-example.py --check
uv run --script scripts/rebuild-alfred-revision-example.py --variant large --check
make verify-real-data-demo
make verify-real-revision-demo
```

These are helper alternatives. The `make` targets wrap the `uv run --script`
commands. They rebuild the checked-in ALFRED/FRED fixtures from public CSV
endpoints and require internet access but no API key.
