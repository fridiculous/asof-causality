# Measurements

This file records the measured result shipped with the artifact. These numbers
are local to the machine and build profile below; they are evidence for this
implementation shape, not universal constants.

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
cargo run -p asof-replay-cli -- replay examples/late-arrival.pipe
```

Output:

```text
prediction_time|prediction_sequence|symbol|signal_value|input_event_ids|max_input_received_time|max_input_sequence
580|3|AAPL|0|-|0|0
590|4|AAPL|1|n1|585|2
610|5|AAPL|1|n1|585|2
620|7|AAPL|-1|c1|615|6
transcript_hash=d869358b32a2f623
labels_seen=1
```

The `check` command also reverses the physical input order and verifies that the
same transcript hash is produced.

## Adversarial Checks

Command:

```text
cargo run -p asof-replay-cli -- check examples/late-arrival.pipe
```

Result:

```text
PASS prefix_equivalence
PASS future_mutation
PASS late_arrival
PASS on_time_vs_late_contrast
PASS correction_append_only
PASS label_separation
PASS deterministic_replay
PASS audit_invariant
```

The most important contrast is `on_time_vs_late_contrast`: moving event `n1`
from received-after to received-before changes the 580 prediction from `0` to
`1`. That proves the engine is using received-time knowledge, not merely
ignoring late events.

## Generated Scenario

Command:

```text
cargo run -p asof-replay-cli -- generate --scenario late-heavy --events 100000 --symbols 1024 --late-rate 0.30 --correction-rate 0.05 --seed 42 --out runs/late-heavy.pipe
```

Result:

```text
generated path=runs/late-heavy.pipe scenario=late-heavy seed=42 data_events=100000 rows=116011 symbols=1024 late_updates=34892 corrections=4993 predictions=10007
```

The generated file is deterministic for seed `42` and physically shuffled by
default in this scenario. It also includes fixed sentinel late-arrival
sequences before the random body, so the on-time-vs-late contrast check has
known adversarial cases. Running `check runs/late-heavy.pipe` samples 32
replay-key cutoffs for the expensive prefix and future-mutation checks and still
exercises the direct late-arrival, correction, label, replay, and audit checks
across the full generated file.

## Leaky Baseline

Command:

```text
cargo run -p asof-replay-cli -- compare-leaky examples/lookahead-negative-control.pipe
```

Expected interpretation:

```text
received-time replay: PASS
observed-time replay (leaky baseline): FAIL
```

The baseline intentionally sorts by `observed_time`. On the negative-control
fixture, it lets a prediction at time `120` use `n_late_positive`, which was not
received until `150`. That prediction is impossible in live replay, and the
audit invariant catches it as
`max_input_replay_key > prediction_replay_key`.

## Throughput

Command:

```text
cargo run --release -p asof-replay-cli -- bench --events 1000000 --symbols 1024
```

Single-run result:

| Representation | Events | Symbols | Elapsed ms | Events/sec |
|---|---:|---:|---:|---:|
| string map | 1,000,000 | 1,024 | 55.725 | 17,945,119 |
| symbol id vec | 1,000,000 | 1,024 | 0.443 | 2,256,063,170 |

The interned-symbol/vector representation was about 126x faster on this
microbenchmark. This does not mean full replay is 126x faster; it isolates one
hot representation decision: map lookup by string versus direct indexed state.

## Surprise

The large swing came from representation, not signal arithmetic. The signal is
just a sentiment value update, so the benchmark mostly measures how expensive it
is to find the per-symbol state slot. That is the useful lesson for this
project: before moving work to a more complicated architecture, make the
point-in-time state representation boring and indexed.

Prediction provenance follows the same lesson. The replay path stores input
provenance as compact inline event keys (`InputSet::Empty` or `InputSet::One`)
and renders those keys back to human-readable event IDs only when producing the
transcript. The v1 built-in signal uses at most one input event per prediction;
a multi-input signal would need a bounded inline set or rolling hash rather than
allocating a `Vec` per prediction.

## Scope Of Conclusions

This measurement supports the correctness and representation claims in this
repo. It does not prove market predictiveness, production data quality,
distributed scale, or trading-system latency. The fixture is synthetic so the
adversarial properties are repeatable and inspectable.
