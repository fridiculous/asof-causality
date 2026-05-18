# Architecture

Crossover is organized around a small deterministic fast path and optional
background workers.

```text
event trace
  -> parser / normalizer
  -> sequencer
  -> deterministic feature engine
  -> admission decision
  -> decision record
  -> latency report

future:
  -> bounded GPU-style worker queue
  -> bounded LLM-style worker queue
  -> logger worker
  -> offline merger/report
```

The current repo implements the baseline event model, replay parser, feature
engine, placement policy, and CLI. The larger project goal is to turn placement
decisions into an admission-control runtime with auditable decision records.

## Current Components

| Component | Crate | Responsibility |
|---|---|---|
| Event model | `crossover-core` | Represents raw and normalized stream events |
| Replay parser | `crossover-core` | Loads deterministic fixture streams |
| Feature engine | `crossover-core` | Maintains simple rolling tick-derived state |
| Placement policy | `crossover-core` | Classifies candidate work by constraints |
| Latency report | `crossover-core` | Produces p50, p95, p99 summaries |
| CLI | `crossover-cli` | Runs replay and synthetic benchmark commands |

## Intended Runtime Shape

The fast path should do only the work needed for immediate event handling:

```text
parse -> normalize -> sequence -> features -> admission decision -> record
```

It should not:

- call an LLM
- call a GPU
- write files or print per event
- wait for a background worker
- use an unbounded queue

Optional work moves to bounded background workers. If a queue is full or a task
is too stale, the system records a drop, defer, or offline decision instead of
slowing down ingest.

## Placement Classes

| Placement | Meaning |
|---|---|
| `HotPathCpu` | Deterministic work safe to run immediately |
| `GpuBatch` | Large numeric work that may benefit from batching |
| `AiSidecar` | Human-facing LLM/model work that must not block ingest |
| `Offline` | Research, reporting, or stale work outside the real-time path |

## Replay And Benchmark Modes

Replay and performance measurement are separate claims.

- Replay mode should prove that fixed input and configuration produce the same
  decisions.
- Benchmark mode should measure real latency and queue pressure. Benchmark
  timings may vary by machine.
