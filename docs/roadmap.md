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
`docs/schemas/audit.schema.json`. JSONL keeps the dependency tree small, works with
basic shell tools, and makes every prediction independently inspectable. The
audit command can run in replay-only mode or compare replay-derived predictions
against stored prediction JSONL, with optional explicit outcome attribution.

The manifest should link generated artifacts and run context, but it should not
pretend that source-control metadata is the audit identity. The durable audit
identity is the data fixture hash, prediction output hash, checks output hash,
transcript hash, and per-record recipe hash. Commit and toolchain metadata are
useful context, not proof that a run is trustworthy.

## Outcome Join Semantics

The current CLI outcome attachment is deliberately strict:
`(symbol, prediction_replay_key)` must match exactly. That is useful for
fixtures, regression tests, and second-pass audits where outcomes are generated
after replay has already produced stable prediction keys. It is not the right
primary API for production outcome data.

Market outcome datasets are normally keyed by economic target identity: symbol,
target timestamp, horizon, close/open convention, and sometimes venue or
corporate-action policy. They should not need to know an internal replay key
before the causality engine has emitted a `PredictionRecord`.

The production outcome adapter should therefore accept target keys such as
`(symbol, target_timestamp, horizon)` or an explicit target event ID, join them
to replay-derived predictions inside the platform, and then write the resolved
`prediction_replay_key` into the final audit artifact. The replay key remains
the immutable audit identity; it should not be the only user-facing join handle.

## Recipe Hashes

`PredictionRecord` stores a bounded inline set of input event keys plus a fixed
BLAKE3 `feature_recipe_hash`. For larger feature sets, the right extension is a
compact recipe digest rather than a growing per-prediction key list. Today the
digest commits to the signal name, signal configuration descriptor, and ordered
input event keys, not the input payload values. Later it can commit to value
hashes, a feature recipe, or snapshot manifest without changing the core
causality invariant.

## Arrow/Parquet Production Boundary

Pipe fixtures and JSONL audit output are the reference interface because they
keep the repository easy to inspect and keep the dependency tree small. They are
not the production-scale I/O boundary. For real quant research workloads,
Parquet is not just an export format; it is the required ingestion and querying
boundary.

The production architecture should be Arrow-native at the adapter boundary and
Parquet-backed at the durable audit/query boundary. Arrow IPC is the natural
ingestion format for typed event batches. Parquet is the storage format for
audited predictions. Together they let the causality engine exchange data with
Polars, DuckDB, Pandas, and Python strategy tooling without repeatedly parsing
text. They also provide schema metadata, predicate pushdown, and column
projection, so researchers can scan millions of audited `PredictionRecord`s
without paying JSONL deserialization cost.

The intended sequence is:

1. Keep pipe text and JSONL plus JSON Schema as the canonical review/debug
   surface.
2. Add typed Arrow IPC ingestion for event batches and prove transcript-hash
   equality against the same pipe fixtures.
3. Add a Parquet audit writer that emits the same `PredictionRecord` contract
   through Arrow `RecordBatch`es.
4. Treat Arrow/Parquet as the production I/O boundary, with JSONL remaining the
   simplest diff and submission format.

The first Arrow/Parquet PR should make this strong claim:

> This PR makes the strong claim that columnar I/O is a first-class production
> boundary, not JSONL reshaped into Parquet: event ingestion uses a typed Arrow
> schema, audit output uses a typed Parquet schema, metadata is explicit, and
> compression is chosen deliberately rather than inherited from parquet-rs
> defaults.

The writer should set compression explicitly. Snappy is the lower-surprise first
choice for interoperability and fast reads. Zstd level 3 is also defensible for
write-once, read-many audit files because the repeated UTF-8 fields should
compress well, but it can remain a later knob.

Parquet file metadata should include:

- `asof.schema`
- `asof.hash_algorithm`
- `asof.hash_encoding`
- `asof.tool`
- `asof.input_commitment`
- fixed-point scale metadata, such as `asof.score_scale` and `asof.bps_scale`

`feature_recipe_hash` remains a per-row column, not file metadata. The metadata
names describe the contract and hashing/tool context; they do not replace the
row-level provenance digest.

The typed schema should avoid treating the export as only JSONL-in-Parquet:

- `symbol_id`: fixed-width integer identity for joins and audit records.
- `symbol`: dictionary-encoded UTF-8 label for display and analyst queries.
- `sentiment`: dictionary-encoded UTF-8 for the low-cardinality sentiment
  fixture domain, including generated mutation markers if they appear in tests.
  It is retained for API coverage, not as the intended production quant feature
  representation.
- `score` and future numeric feature columns: fixed-point decimal or integer
  physical types, not text payload parsing in the replay hot path.
- `return_bps`: current audit JSONL stores this as a JSON number. A typed
  Parquet adapter should prefer integer basis points (`Int64`) or an Arrow
  decimal type over `Float64`.
- `feature_recipe_hash`: reviewer-friendly hex `Utf8` first, with
  `asof.hash_encoding = "hex"`. Production pipelines may prefer
  `FixedSizeBinary(32)` for the same BLAKE3 bytes.
- `payload`: only keep this as an optional compatibility column. If typed
  columns exist, they should be the adapter contract and the pipe payload should
  be reconstructed at the boundary rather than treated as the source of truth.

Sequencing should stay explicit. Land a thin payload-`Utf8` Parquet adapter
first if typed feature columns would expand the scope too far, then promote to
typed feature columns in a follow-up. Do not bundle storing parsed
`FeatureValues` on `Event` into the Arrow work; that is a separate
architectural cleanup.

## Out-Of-Core Replay And Check Scaling

The v1 CLI is a deterministic batch harness. It reads pipe events into memory,
sorts the full vector by `(received_time, received_sequence_number, event_id)`,
and then replays. That is acceptable for inspectable fixtures and adversarial
benchmarks, but it is not a streaming architecture for 50GB tick files.

The production ingestion path should require Arrow/Parquet row groups sorted by
received-time order, or at least sorted closely enough that replay can use a
bounded reorder buffer. Late arrivals can then be handled with a min-heap or
watermark policy instead of a global in-memory sort. Physical row shuffling
should remain a regression test for determinism, not the expected production
layout.

The same distinction applies to adversarial checks. The replay state update path
is fast, but prefix-equivalence and future-mutation checks intentionally rerun
or mutate multiple prefixes. The default 32-cutoff sample is a deterministic
large-input guardrail, not a claim that the falsification harness is fully
incremental. A production checker should reuse prefix transcripts, avoid deep
whole-log clones, and turn these checks into incremental commitments over
sorted input partitions.

## Strategy Layer Handoff: Python Bindings And IPC

This repository stops at the signal layer and emits audited
`PredictionRecord`s. Downstream strategies are a different system: they consume
signal streams, maintain portfolio state, apply risk controls, size orders, and
score fills. In most quant teams, that downstream research and strategy work is
Python-first.

The Rust kernel should therefore expose a stable handoff boundary rather than
forcing strategy authors to work inside the CLI. Two evolution paths are
compatible with the current crate split:

- Zero-copy FFI: package the replay engine as a Python wheel with PyO3 and
  Arrow/Polars-compatible buffers. Researchers should be able to pass
  Arrow-backed historical data from Python into Rust replay and receive an
  audited Polars DataFrame or Arrow table back with minimal serialization.
- Operational IPC: for live, paper-trading, or batch platform integration, run
  the Rust causality cage as a separate process upstream of Python strategy
  daemons. Unix domain sockets, gRPC over TCP, or another small framed protocol
  can stream evaluated `SignalEvaluation`s or finalized `PredictionRecord`s
  while keeping the strict as-of boundary in Rust.

The strategic split is intentional: Rust owns deterministic replay and
causality enforcement; Python owns rapid research iteration, portfolio logic,
and strategy analytics over the certified prediction stream.
