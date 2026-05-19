# Architecture

```text
generator or pipe fixture
  -> parse Event
  -> sort by (received_time, sequence)
  -> apply data events to internal StateStore
  -> call Signal::predict(AsOfView, prediction_time)
  -> append PredictionRecord
  -> hash deterministic transcript
```

## Core Concepts

| Concept | Responsibility |
|---|---|
| `Event` | Two-clock input row with stable `event_id`, symbol, kind, and payload |
| `StateStore` | Internal mutable state created from received events |
| `AsOfView` | Public opaque read-only view exposed to signal code |
| `Signal` | Restricted API over `AsOfView`, never the full event list |
| `PredictionRecord` | Immutable audit record with input event provenance |
| `PredictionLog` | Append-only prediction transcript plus deterministic hash |
| `ReplayEngine` | Orders events, updates state, emits predictions, and computes labels later |
| `Generator` | Creates deterministic late-arrival/correction fixtures from a seed |

`StateStore` and `StateWriter` are crate-private. Signal authors can query
`AsOfView`, but cannot construct it, mutate it, or access the full event list
through the signal API.

## Event Kinds

| Kind | Behavior |
|---|---|
| `news` | Updates per-symbol sentiment state from payload |
| `correction` | Append-only correction event with its own received time |
| `predict` | Emits a prediction for the symbol at this received time |
| `label` | Optional future label data; excluded from prediction state |

## Correctness Boundary

For each prediction:

```text
max_input_replay_key <= prediction_replay_key
where replay_key = (received_time, sequence)
```

Late events may create new future predictions, but they cannot mutate old
prediction records. Corrections are also append-only events; a correction
received at 10:15 cannot affect a prediction emitted at 09:45.

## Start-To-Finish Flow

`run-suite` wires the artifact together:

```text
GenerateConfig(seed, scenario, late_rate, correction_rate)
  -> GeneratedStream(events.pipe)
  -> ReplayEngine(predictions.pipe)
  -> adversarial checks(checks.txt)
  -> summary.md with transcript hash and check results
```

Generated fixtures are deterministic for a fixed seed. The `late-heavy`
scenario also shuffles physical file order so deterministic replay is exercised
against out-of-order input rather than only a hand-written toy fixture.

## Negative Control

The CLI also exposes a deliberately broken replay order:

```text
received-time replay: sort by (received_time, sequence)
observed-time baseline: sort by (observed_time, sequence)
```

The observed-time baseline is not a production mode. It exists so
`compare-leaky` can run the same fixture through both engines and show the exact
impossible prediction a naive backtest would emit.
