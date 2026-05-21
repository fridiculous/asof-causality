# Success Criteria

asof-causality succeeds when it can prove that identical received-event prefixes
produce identical predictions.

## Required Checks

- Prefix equivalence: full replay and `received_time <= T` replay produce the
  same predictions at or before `T`.
- Future mutation: mutating every event after `T` cannot change predictions at
  or before `T`.
- Late arrival: an event observed before prediction time but received after it
  is not used by that prediction.
- On-time vs late contrast: moving the same event from received-before to
  received-after prediction time can change the prediction.
- Feature-correction handling: feature corrections are append-only and cannot
  rewrite old prediction records.
- Outcome separation: removing outcome computation does not change predictions.
- Deterministic replay: shuffled physical input produces the same transcript
  hash.
- Audit invariant: every prediction has
  `max_input_replay_key <= prediction_replay_key`.

The core test suite runs these checks exhaustively against the small adversarial
fixture. The CLI uses deterministic cutoff sampling for large generated files so
the same checks remain practical on six-figure event streams; `--exhaustive`
keeps the full sweep available when the input size is appropriate.

## End-To-End Evidence

`run-suite` succeeds when it can generate an adversarial fixture from a seed,
replay it, emit prediction records, run the checks, and write a summary report
with the transcript hash. It also writes a `manifest.json` that links the data
fixture, signal, check output, toolchain, invocation, UTC run timestamp, source
commit context, workspace dirty flag, transcript hash, and check counts. This
makes the submission a complete workflow rather than only a library API.

`audit` succeeds when it emits schema-versioned JSONL records described by
`docs/audit.schema.json` and every record reports `causally_valid: true`. When a
stored prediction JSONL file is supplied, every replay-derived prediction must
match the stored prediction at `(symbol, prediction_replay_key)` and, by
default, must match `feature_recipe_hash`. Explicit outcomes may be attached to
audit records as values, but scoring remains downstream. The JSONL surface is
the lean machine-readable audit contract; richer adapters such as Parquet are
downstream exports.

`negative-control` succeeds as a demonstration when the received-time engine emits
zero impossible predictions and the observed-time baseline emits at least one on
the negative-control fixture. This is the visible proof that the checks catch a
real class of naive backtest error.

The windowed built-in signal succeeds when a prediction can cite multiple
feature inputs while still satisfying the same audit invariant. This proves the
provenance model is signal-shaped rather than a one-row special case.

## Performance Evidence

The benchmark should report single-node replay throughput for:

- string-keyed map state
- dense symbol-slot vector state

These measurements show how representation choices affect point-in-time replay
cost without claiming production trading latency.
