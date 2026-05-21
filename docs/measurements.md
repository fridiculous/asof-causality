# Measurements

This file records the measured result shipped with the artifact. These numbers
are local to the machine and build profile below; they are evidence for this
implementation shape, not universal constants.

The command names in this file are current for the `asof-cli` package and the
installed `asof` binary. The throughput table is a recorded single-run
microbenchmark; rerun `make bench` before using it as a fresh performance claim.

## Machine

```text
Machine: MacBook Pro, Apple M5 Pro, 18 cores, 48 GB memory
OS: macOS 26.3, Darwin 25.3.0, arm64
Rust: rustc 1.95.0 (59807616e 2026-04-14)
Cargo: cargo 1.95.0 (f2d3ce0bd 2026-03-21)
Build: release for bench, dev for replay/check
```

## Transcript Hash

Command:

```text
cargo run -p asof-cli -- replay examples/late-arrival.pipe
```

Output:

```text
replay path=examples/late-arrival.pipe signal=last-feature-sentiment events=7
prediction_replay_key|symbol|signal_value|input_event_ids|max_input_replay_key
580:3:p1|AAPL|0|-|-
590:4:p2|AAPL|1|n1|585:2:n1
610:5:p3|AAPL|1|n1|585:2:n1
620:7:p4|AAPL|-1|c1|615:6:c1
transcript_hash=d959650f0492c42e
outcomes_seen=1
```

The `check` command also reverses the physical input order and verifies that the
same transcript hash is produced.

## Adversarial Checks

Command:

```text
cargo run -p asof-cli -- check examples/late-arrival.pipe
```

Result:

```text
asof check
  fixture    examples/late-arrival.pipe
  events     7
  signal     last-feature-sentiment
  cutoffs    all 4 (max 32)

ADVERSARIAL CHECKS                                         8/8 PASS
  [PASS]  prefix_equivalence               all received-time prefixes matched full replay
  [PASS]  future_mutation                  mutating future rows did not change prior PredictionRecords
  [PASS]  late_arrival                     late events were not used before their replay key
  [PASS]  on_time_vs_late_contrast         moving n1 earlier changed SignalEvaluation at 580 from 0 to 1
  [PASS]  feature_correction_append_only   feature corrections did not rewrite prior PredictionRecords
  [PASS]  outcome_separation               disabling outcomes did not change PredictionRecords
  [PASS]  deterministic_replay             shuffled input produced same transcript hash
  [PASS]  audit_invariant                  all PredictionRecords satisfy max_input_replay_key <= prediction_replay_key

PROVENANCE
  transcript_hash      d959650f0492c42e
  predictions_emitted  4
  outcomes_separated   1
```

The most important contrast is `on_time_vs_late_contrast`: moving event `n1`
from received-after to received-before changes the 580 `SignalEvaluation` from
`0` to `1`. That proves the engine is using received-time knowledge, not merely
ignoring late events.

## Generated Scenario

Command:

```text
cargo run -p asof-cli -- generate --scenario late-heavy --events 100000 --symbols 1024 --late-rate 0.30 --feature-correction-rate 0.05 --seed 42 --out runs/late-heavy.pipe
```

Result:

```text
generated path=runs/late-heavy.pipe scenario=late-heavy seed=42 data_events=100000 rows=116008 symbols=1024 late_updates=34891 feature_corrections=4993 predictions=10005
```

The generated file is deterministic for seed `42` and physically shuffled by
default in this scenario. It also includes a fixed sentinel late-arrival
received sequence before the random body, so the on-time-vs-late contrast check has a
known adversarial case. Running `check runs/late-heavy.pipe` samples 32
received-time cutoffs for the expensive prefix and future-mutation checks and
still exercises the direct late-arrival, feature-correction, outcome, replay, and audit
checks across the full generated file.

## Leaky Baseline

Command:

```text
cargo run -p asof-cli -- negative-control examples/lookahead-negative-control.pipe --signal windowed-feature-sentiment
```

Expected interpretation:

```text
asof negative-control
  fixture  examples/lookahead-negative-control.pipe
  events   12
  signal   windowed-feature-sentiment

ENGINE A: received-time replay (correct)
  ordering             (received_time, received_sequence_number, event_id)
  transcript_hash      ed03706f6f79c31f
  impossible           0
  VERDICT              PASS

ENGINE B: observed-time replay (deliberately broken baseline)
  ordering             (observed_time, received_sequence_number, event_id)
  transcript_hash      f7b67d321cac694e
  impossible           3
  VERDICT              FAIL

LEAKED PREDICTION RECORDS (engine B)

  p_before_same_time_sequence at (95, 4, p_before_same_time_sequence)
    signal_value     0
    leaked_input     n_same_time_later  at (95, 5, n_same_time_later)
    violation        input received_sequence_number > prediction received_sequence_number at same received_time
    interpretation   prediction at t=95 used same-timestamp event that sorts after it

  p_before_late_feature at (120, 6, p_before_late_feature)
    signal_value     1
    leaked_input     n_late_positive    at (150, 7, n_late_positive)
    violation        input replay key > prediction replay key by delta=30
    interpretation   prediction at t=120 used event that arrived at t=150

  p_before_correction at (170, 10, p_before_correction)
    signal_value     1
    leaked_input     c_late_negative    at (180, 9, c_late_negative)
    violation        input replay key > prediction replay key by delta=10
    interpretation   prediction at t=170 used correction received at t=180

DIAGNOSTIC
  the broken engine produced 3 impossible PredictionRecords across 3 distinct leak classes
  the correct engine produced 0 impossible PredictionRecords
  the audit invariant catches the failure mode the engine is designed to prevent
```

The baseline intentionally sorts by `observed_time`. On the negative-control
fixture, it lets a prediction at replay key `95:4:p_before_same_time_sequence`
use `n_same_time_later`, which has the same `received_time` but a later received sequence.
It also lets later predictions use records received at `150` and `180`. Those
predictions are impossible in live replay, and the audit invariant catches them
as `max_input_replay_key > prediction_replay_key`.

## Throughput

Command:

```text
cargo run --release -p asof-cli -- bench --events 1000000 --symbols 1024
```

Single-run result:

| Representation | Events | Symbols | Elapsed ms | Events/sec |
|---|---:|---:|---:|---:|
| string map | 1,000,000 | 1,024 | 55.725 | 17,945,119 |
| symbol slot vec | 1,000,000 | 1,024 | 0.443 | 2,256,063,170 |

The dense symbol-slot vector representation was about 126x faster on this
microbenchmark. This does not mean full replay is 126x faster; it isolates one
hot representation decision: map lookup by string versus direct indexed state.
Production replay still pays for catalog validation and event slotting before
the hot loop, so its end-to-end speedup should be smaller than this microbench.

## Surprise

The large swing came from representation, not signal arithmetic. The signal is
just a sentiment value update, so the benchmark mostly measures how expensive it
is to find the per-symbol state slot. That is the useful lesson for this
project: before moving work to a more complicated architecture, make the
point-in-time state representation boring and indexed.

The replay implementation now applies that lesson directly. `Event` keeps the
human symbol string for input and transcript rendering, replay builds a
collision-checked symbol catalog once, `StateStore` uses dense `SymbolSlot`
indexes, and `PredictionRecord` keeps the stable `SymbolId` for audit output.
Prediction provenance follows the same shape: input provenance is stored as
compact inline event keys (`InputSet::Empty`, `InputSet::One`, or fixed-capacity
`InputSet::Many`) and rendered back to human-readable event IDs only when
producing the transcript. The windowed built-in signal uses that bounded inline
set so multi-input provenance does not allocate a `Vec` per prediction.

## Scope Of Conclusions

This measurement supports the correctness and representation claims in this
repo. It does not prove market predictiveness, production data quality,
distributed scale, or trading-system latency. The fixture is synthetic so the
adversarial properties are repeatable and inspectable.
