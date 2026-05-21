# Success Criteria

asof-causality succeeds when it can prove that identical received-event prefixes
produce identical `PredictionRecord` transcripts.

## Required Properties

- Receipt-time causality: full replay and `received_time <= T` replay agree at
  each prefix, future-row mutations do not change prior records, late events are
  not used early, and every record satisfies
  `max_input_replay_key <= prediction_replay_key`.
- On-time vs late contrast: when a fixture plants a late event that can affect
  an in-between prediction, moving that event from received-after to
  received-before prediction time can change the resulting `SignalEvaluation`.
- Feature-correction handling: feature corrections are append-only and cannot
  rewrite old `PredictionRecord`s.
- Outcome separation: removing outcome computation does not change
  `PredictionRecord`s.
- Deterministic replay: shuffled physical input produces the same transcript
  hash.

The implementation exercises those five properties through eight check methods:
`prefix_equivalence`, `future_mutation`, `late_arrival`,
`on_time_vs_late_contrast`, `feature_correction_append_only`,
`outcome_separation`, `deterministic_replay`, and `audit_invariant`. The
contrast method is a fixture-sensitivity check: if the fixture does not contain
a planted late event that changes an in-between prediction, it reports that case
as not applicable rather than treating the signal as invalid.

The core test suite runs these methods exhaustively against small adversarial
fixtures. The CLI uses deterministic cutoff sampling for large generated files
so the methods remain practical on six-figure event streams; `--exhaustive`
keeps the full sweep available when the input size is appropriate.

## End-To-End Evidence

`run-suite` succeeds when it can generate an adversarial fixture from a seed,
replay it, append `PredictionRecord`s, run the checks, and write a summary report
with the transcript hash. It also writes a `manifest.json` that links the data
fixture, signal, check output, toolchain, invocation, UTC run timestamp, source
commit context, workspace dirty flag, transcript hash, and check counts. This
makes the submission a complete workflow rather than only a library API.

`audit` succeeds when it emits schema-versioned JSONL records described by
`schemas/audit.schema.json` and every record reports `causally_valid: true`.
When a stored prediction JSONL file is supplied, every replay-derived prediction
must match the stored prediction at `(symbol, prediction_replay_key)` and, by
default, must match `feature_recipe_hash`. Explicit outcomes may be attached to
audit records as values, but scoring remains downstream. The JSONL surface is
the lean machine-readable audit contract; richer adapters such as Parquet are
downstream exports. A Parquet adapter succeeds only if it is a typed, columnar
audit contract with explicit schema metadata and deliberately chosen
compression, not just JSONL reshaped into a columnar container.

`negative-control` succeeds as a demonstration when the received-time engine
produces zero impossible `PredictionRecord`s and the observed-time baseline
produces at least one on the negative-control fixture. This is the visible
proof that the checks catch a real class of naive backtest error.

The windowed built-in signal in `asof-signals` succeeds when a replay-derived
`PredictionRecord` can cite multiple feature inputs while still satisfying the
same audit invariant. This proves the provenance model is signal-shaped rather
than a one-row special case.

## Performance Evidence

The benchmark should report single-node replay throughput for:

- string-keyed map state
- dense symbol-slot vector state

These measurements show how representation choices affect point-in-time replay
cost without claiming production trading latency.
