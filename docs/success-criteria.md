# Success Criteria

Crossover succeeds when it produces useful event decision records and proves
that optional enrichment cannot compromise the fast path.

## Baseline

- The repo builds with a stock Rust toolchain.
- `cargo run -p crossover-cli -- replay examples/events.pipe` processes fixture
  events end to end.
- `cargo run -p crossover-cli -- bench 100000` reports throughput and latency.
- `cargo test` covers parsing, latency summaries, policy decisions, and feature
  updates.

## Decision Integrity Goal

The larger project should emit clear records showing:

- what event arrived
- which deterministic features changed
- what optional work was considered
- what was admitted, skipped, deferred, or marked offline
- why that decision was made
- whether background work completed before its deadline
- how much useful value was retained or lost

## Fast Path Invariants

- Same replay input and configuration produce the same decisions.
- The fast path does not require network access, an LLM, a GPU, or external
  market data.
- Optional worker failure does not stop event processing.
- Queues are bounded.
- Full queues produce explicit drop, defer, or offline decisions instead of
  blocking.

## Evidence

Reports should include:

- event count and throughput
- p50, p95, p99, and max latency
- decision counts by placement/action
- background worker queue depth, drops, timeouts, and lag once workers exist
- retained urgent value once admission control exists
- replay hash for deterministic replay mode

## Seriousness Test

The project should avoid claims such as "AI predicts markets" or "GPU is always
faster." It should instead show when optional work is worth doing, when it should
be skipped, and what evidence proves the fast path stayed protected.
