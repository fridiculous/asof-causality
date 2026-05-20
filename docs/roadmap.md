# Roadmap

This repository is the causality kernel: it proves whether a prediction used
only data available at its replay key. It intentionally stops before strategy
simulation, portfolio state, or research-notebook ergonomics.

## Kernel Before Strategy

The current boundary is `Event -> AsOfView -> Signal -> PredictionRecord`.
Signal code can read only the opaque as-of view and emits immutable prediction
records with input provenance. A future strategy layer should consume those
prediction records through its own point-in-time `StrategyView` and emit
separate `DecisionRecord`s. Keeping those layers split preserves the simple
invariant this repo proves:

```text
max_input_replay_key <= prediction_replay_key
```

## Audit Surface

The submission-facing surface is JSONL audit output plus
`docs/audit.schema.json`. JSONL keeps the dependency tree small, works with
basic shell tools, and makes every prediction independently inspectable. The
audit command can run in replay-only mode or compare replay-derived predictions
against stored prediction JSONL, with optional explicit outcome attribution.

The manifest should link generated artifacts and run context, but it should not
pretend that source-control metadata is the audit identity. The durable audit
identity is the data fixture hash, prediction output hash, checks output hash,
transcript hash, and per-record recipe hash. Commit and toolchain metadata are
useful context, not proof that a run is trustworthy.

## Recipe Hashes

`PredictionRecord` stores a bounded inline set of input event keys plus a fixed
BLAKE3 `feature_recipe_hash`. For larger feature sets, the right extension is a
compact recipe digest rather than a growing per-prediction key list. Today the
digest commits to the signal name and ordered input event keys, not the input
payload values or signal configuration. Later it can commit to a feature recipe
or snapshot manifest without changing the core causality invariant.

## Parquet Adapter

Parquet is a good downstream adapter, not the first submission surface. It would
improve Polars, Pandas, DuckDB, and Jupyter workflows by carrying an embedded
schema and columnar layout. It also adds Arrow/Parquet dependency weight and
compile-time surface area. The intended sequence is:

1. Keep JSONL plus JSON Schema as the canonical audit contract.
2. Add a Parquet writer that adapts the same audit records.
3. Treat Parquet files as ergonomic exports, with JSONL remaining the simplest
   review and diff format.

## Deferred Engine Work

The benchmark shows that dense symbol-indexed state is the right direction for
large replay workloads. The current replay path already uses stable `SymbolId`
values in records and state keys, while the benchmark demonstrates the denser
vector-backed representation. Moving the production `StateStore` to that shape
is worthwhile after the audit contract is stable.
