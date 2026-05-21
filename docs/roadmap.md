# Roadmap

This repository is the causality kernel: it proves whether a prediction used
only data available at its replay key. It intentionally stops before strategy
simulation, portfolio state, or research-notebook ergonomics.

## Kernel Before Strategy

The current boundary is
`Event -> ReplayKey -> AsOfView -> Signal::evaluate(as_of_timestamp) -> SignalEvaluation -> PredictionRecord`.
Signal code can read only the opaque as-of view and returns a
`SignalEvaluation` that the replay engine converts into an immutable
`PredictionRecord` with input provenance. A future strategy layer should
consume those `PredictionRecord`s through its own point-in-time `StrategyView`
and emit separate `DecisionRecord`s. Keeping those layers split preserves the
simple invariant this repo proves:

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
digest commits to the signal name, signal configuration descriptor, and ordered
input event keys, not the input payload values. Later it can commit to value
hashes, a feature recipe, or snapshot manifest without changing the core
causality invariant.

## Parquet Adapter

Parquet is a good downstream adapter, not the first submission surface. It would
improve Polars, Pandas, DuckDB, and Jupyter workflows by carrying an embedded
schema and columnar layout. It also adds Arrow/Parquet dependency weight and
compile-time surface area. The intended sequence is:

1. Keep JSONL plus JSON Schema as the canonical audit contract.
2. Add a Parquet writer that adapts the same audit records.
3. Treat Parquet files as ergonomic exports, with JSONL remaining the simplest
   review and diff format.

The first Arrow/Parquet PR should make this strong claim:

> This PR makes the strong claim that the audit export is no longer just JSONL
> reshaped into Parquet: it is a typed, columnar audit contract with explicit
> schema metadata, stable provenance columns, and compression chosen
> deliberately rather than inherited from parquet-rs defaults.

The writer should set compression explicitly. Snappy is the lower-surprise first
choice for interoperability and fast reads. Zstd level 3 is also defensible for
write-once, read-many audit files because the repeated UTF-8 fields should
compress well, but it can remain a later knob.

Parquet file metadata should include:

- `asof.schema`
- `asof.hash_algorithm`
- `asof.tool`

`feature_recipe_hash` remains a per-row column, not file metadata. The metadata
names describe the contract and hashing/tool context; they do not replace the
row-level provenance digest.

The typed schema should avoid treating the export as only JSONL-in-Parquet:

- `sentiment`: `Dictionary<Int32, Utf8>` for the low-cardinality sentiment
  domain, including generated mutation markers if they appear in fixtures.
- `return_bps`: revisit before implementation. If PR #11 lands `FixedDecimal`,
  use the same decimal discipline for outcomes. Otherwise prefer integer basis
  points (`Int64`) or an Arrow decimal type over `Float64`.
- `payload`: only keep this as `Utf8` for a thin first adapter if the typed
  feature schema is blocked.

Sequencing should stay explicit. Block on PR #11 only if it lands quickly and
its `FeatureDType` shape is stable. If it slips, land a thin payload-`Utf8`
Parquet adapter first to prove the Arrow boundary, then promote to typed feature
columns in a follow-up. Do not bundle storing parsed `FeatureValues` on `Event`
into the Arrow work; that is a separate architectural cleanup.

## Engine State Representation

The benchmark shows that dense symbol-indexed state is the right direction for
large replay workloads. Production replay now builds a collision-checked symbol
catalog before the hot loop, keeps stable `SymbolId` values in audit records,
and uses replay-local `SymbolSlot` values to index a vector-backed
`StateStore`.
