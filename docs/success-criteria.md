# Success Criteria

asof-replay succeeds when it can prove that identical replay-key prefixes
produce identical predictions. A replay key is `(received_time, sequence)`.

## Required Checks

- Prefix equivalence: full replay and replay-key prefix replay produce the same
  predictions at or before that key.
- Future mutation: mutating every event after a replay key cannot change
  predictions at or before that key.
- Late arrival: an event observed before prediction time but received after it
  is not used by that prediction.
- On-time vs late contrast: moving the same event from received-before to
  received-after prediction time can change the prediction.
- Correction handling: corrections are append-only and cannot rewrite old
  prediction records.
- Label separation: removing label computation does not change predictions.
- Deterministic replay: shuffled physical input produces the same transcript
  hash.
- Audit invariant: every prediction has
  `max_input_replay_key <= prediction_replay_key`.

The core test suite runs these checks exhaustively against the small adversarial
fixture. The CLI uses deterministic replay-key cutoff sampling for large
generated files so the same checks remain practical on six-figure event streams;
`--exhaustive` keeps the full sweep available when the input size is
appropriate.

Seven universal leakage checks are also property-tested over randomly generated
bitemporal-valid event streams. The eighth check, `on_time_vs_late_contrast`,
is a positive control on the test material itself: it asserts that curated and
generated adversarial streams contain late events whose receipt order can change
a prediction. The generator has its own property test requiring every
`late-heavy` seed to emit multiple structural late-contrast opportunities.

## End-To-End Evidence

`run-suite` succeeds when it can generate an adversarial fixture from a seed,
replay it, emit prediction records, run the checks, and write a summary report
with the transcript hash. This makes the submission a complete workflow rather
than only a library API.

`compare-leaky` succeeds as a demonstration when the received-time engine emits
zero impossible predictions and the observed-time baseline emits at least one on
the negative-control fixture. This is the visible proof that the checks catch a
real class of naive backtest error.

## Performance Evidence

The benchmark should report single-node replay throughput for:

- string-keyed map state
- interned symbol-ID vector state

These measurements show how representation choices affect point-in-time replay
cost without claiming production trading latency.
