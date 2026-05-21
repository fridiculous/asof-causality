# Architecture

```text
generator or pipe fixture
  -> parse Event
  -> intern symbols into stable SymbolId plus dense SymbolSlot
  -> sort by (received_time, sequence, event_id)
  -> apply data events to internal StateStore
  -> call Signal::predict(AsOfView, symbol_slot, prediction_time)
  -> append PredictionRecord
  -> hash deterministic transcript
```

## Core Concepts

| Concept | Responsibility |
|---|---|
| `Event` | Two-clock input row with stable `event_id`, human symbol, derived `SymbolId`, role, and payload |
| `FeatureDType` | Declares deterministic feature value representation, not ML modeling semantics |
| `StateStore` | Internal mutable state created from received events |
| `AsOfView` | Public opaque read-only view exposed to signal code |
| `Signal` | Restricted API over `AsOfView`, never the full event list |
| `PredictionRecord` | Immutable audit record with input event provenance |
| `PredictionLog` | Append-only prediction transcript plus deterministic hash |
| `ReplayEngine` | Orders events, updates state, emits predictions, and computes outcomes later |
| `Generator` | Creates deterministic late-arrival/feature-correction fixtures from a seed |

`StateStore` and `StateWriter` are crate-private. Signal authors receive a
replay-local `SymbolSlot` and can query `AsOfView`, but cannot construct it,
mutate it, or access the full event list through the signal API. The default
`last-feature-sentiment` signal records one input event. The
`windowed-feature-sentiment` signal records a bounded
inline set of recent feature inputs, proving the provenance path is not limited
to one-row examples. The `windowed-zscore` signal reads `score=...` fields with
`FeatureDType::FixedDecimal { scale: 6 }` through the same opaque view and
buckets the latest rolling Z-score to `-1`, `0`, or `1`, showing that the kernel
is not sentiment-coupled.
`vol-adjusted-momentum` implements a fixed-parameter fast/slow moving-average
crossover gated by realized volatility.

Built-in feature schema is intentionally small: `sentiment` has dtype `Text`
and `score` has dtype `FixedDecimal { scale: 6 }`. There is no separate
`FeatureValueKind` such as continuous or categorical in the core contract yet;
that would be downstream modeling metadata, not required for point-in-time
replay.

Numeric `score=...` payloads are parsed according to that fixed-decimal dtype
into `FixedDecimal`, a signed scaled integer with six decimal places. Numeric
replay decisions are integer deterministic: the Z-score threshold comparison
uses squared integer arithmetic instead of `sqrt`, the momentum signal uses
integer moving averages and mean absolute deviation. Benchmark throughput
reporting may format rates with floats, but prediction transcripts do not
depend on floating-point arithmetic.

For a public real-data demonstration of the same bitemporal boundary on
ALFRED/FRED source data, see `docs/real-data-demo.md`.

`InputSet::Many` stores up to eight event keys inline. That cap is deliberate:
it keeps prediction records fixed-size and allocation-free in the replay path.
Signals that need larger provenance should use a separate compact recipe hash or
snapshot manifest rather than growing per-prediction heap state.

The CLI `audit` command renders those records as JSONL. The public shape is
documented in `docs/audit.schema.json`. The JSONL audit record includes a BLAKE3
`feature_recipe_hash`, `causally_valid`, optional
`matched_stored_prediction`, and optional outcome attribution. Stored
predictions are matched by `(symbol, prediction_replay_key)`. Outcomes must
explicitly name the prediction replay key; the kernel attaches outcome values
but does not score them.

The current `feature_recipe_hash` is intentionally an input-set commitment. It
commits to the signal name, signal configuration descriptor, and ordered input
event keys. It does not separately commit to event payload values or replay
ordering metadata. Later schema versions can commit to fuller feature recipes or
input-value snapshots without changing the causality invariant.

Symbols follow the same hot-path shape. `Event` keeps the original symbol string
for input and transcript rendering. Replay builds a symbol catalog once,
rejecting `SymbolId` collisions instead of merging labels. `StateStore` is a
dense `Vec` indexed by replay-local `SymbolSlot`, while `PredictionRecord`
continues to store the stable `SymbolId` used by audit output.

This artifact stops at the signal layer. A full strategy layer would consume
`PredictionRecord`s through its own point-in-time `StrategyView`, maintain
portfolio state, and emit immutable `DecisionRecord`s with cross-strategy
isolation checks. That is the natural next layer, but it is intentionally out of
scope for this repository.

It also intentionally avoids PnL, position tracking, fills, slippage, and market
impact. Those belong to portfolio simulation and scoring. This kernel emits
causal prediction and audit records that downstream tools can score without
expanding the verifier into a backtester.

## Event Roles

| Role | Behavior |
|---|---|
| `feature` | Updates per-symbol feature state from declared payload fields such as `sentiment=...` or `score=...` |
| `feature_correction` | Append-only feature correction with its own received time |
| `prediction` | Emits a prediction for the symbol at this received time |
| `outcome` | Optional future outcome data; excluded from prediction state |

## Correctness Boundary

For each prediction:

```text
max_input_replay_key <= prediction_replay_key
```

The replay key is `(received_time, sequence, event_id)`, not just time. Late
events may create new future predictions, but they cannot mutate old prediction
records. Same-timestamp events with a later sequence are also future inputs for
an earlier prediction at that timestamp. Feature corrections are append-only
events; a feature correction received at replay key `(10:15, 9, c1)` cannot
affect a prediction emitted at `(10:15, 8, p1)`.

### Why There Is No Safety-Lag Option

The core contract intentionally proves exact receipt-time causality:

```text
input.received_time <= prediction_time
```

A production platform may choose a stricter availability rule such as:

```text
input.received_time <= prediction_time - safety_lag
```

That is useful when vendor timestamps are noisy, batch pipelines settle after a
delay, or teams want conservative post-close attribution. This artifact does not
expose `safety_lag` as a replay option because a naive lag filter can be
causally safe but semantically wrong with bounded state.

For example, if too-fresh events are applied to a bounded recent-feature array
before filtering, they can evict older events that were still eligible under the
lagged cutoff. A post-hoc audit would see no forbidden inputs and pass, while the
lagged replay would no longer represent the state the model should have known.

Conservative lags can still be modeled explicitly by shifting feature
`received_time` values forward before replay. A first-class safety-lag mode would
need to be implemented as delayed availability, a cutoff-aware `AsOfView` over
sufficient history, or bounded history with explicit overflow/fail-closed
behavior.

## Start-To-Finish Flow

`run-suite` wires the artifact together:

```text
GenerateConfig(seed, scenario, late_rate, feature_correction_rate)
  -> GeneratedStream(events.pipe)
  -> ReplayEngine(predictions.pipe)
  -> adversarial checks(checks.txt)
  -> summary.md with transcript hash and check results
  -> manifest.json run certificate with hash-linked run identity
```

Generated fixtures are deterministic for a fixed seed. The `late-heavy`
scenario also shuffles physical file order so deterministic replay is exercised
against out-of-order input rather than only a hand-written toy fixture.

The manifest is the run certificate for the output directory. It records the
scenario, signal, invocation, UTC run timestamp, source commit context,
workspace dirty flag, Rust toolchain, hash algorithm, fixture hash, prediction
output hash, checks output hash, final transcript hash, and check pass/fail
counts. It is meant to make a run verifiable from artifacts rather than from
prose; commit metadata is context, not the audit identity.

## Negative Control

The CLI also exposes a deliberately broken replay order:

```text
received-time replay: sort by (received_time, sequence, event_id)
observed-time baseline: sort by (observed_time, sequence, event_id)
```

The observed-time baseline is not a production mode. It exists so
`negative-control` can run the same fixture through both engines and show the exact
impossible prediction a naive backtest would emit.
