# Crossover

Crossover is a real-time financial event triage experiment.

It answers one practical systems question:

> When a financial event arrives, what work should happen immediately, what work
> can happen later, and what should be skipped?

The goal is not to build a trading bot or an AI analyst. The goal is to build a
small, measured system that keeps the fast path deterministic and low-latency
while deciding which optional GPU-shaped or LLM-shaped work is worth admitting
to background workers.

## What This Builds

The current baseline contains:

- a Rust workspace with core pipeline and CLI crates
- normalized events for ticks, news, filings, and alternative data
- deterministic fixture replay
- simple tick-derived features
- explicit placement decisions for CPU, GPU batch, AI/LLM sidecar, and offline
- latency summaries for replay and synthetic benchmark runs
- docs for the larger Decision Integrity project goal

The next step is an admission controller that produces auditable decision
records while proving that slow background work cannot block the hot path.

## Why It Matters

Financial event systems often benefit from expensive enrichment:

- explain why an alert fired
- summarize a filing
- classify breaking news
- compute large numeric batches
- run offline research or backtests

Those tasks are useful, but they do not all belong in the same runtime path.
Crossover treats placement as a measured decision. It keeps urgent,
replayable work on the fast path and routes optional work only when the system
can afford it.

## Quick Start

```sh
cargo run -p crossover-cli -- replay examples/events.pipe
cargo run -p crossover-cli -- bench 100000
cargo test
```

No network, LLM provider, market data feed, GPU, or API key is required for the
baseline.

## Commit Style

This repo uses Conventional Commits. See [CONTRIBUTING.md](CONTRIBUTING.md) for
the commit message format.

## Project Goal

The first-pass project goal is documented in
[docs/project-outline.md](docs/project-outline.md).

In short:

> Build a clear Decision Integrity layer for real-time financial events. For
> each event, emit an audit record explaining what happened, what work was
> admitted or skipped, why that decision was made, and whether the fast path
> stayed within its latency budget.

## Repository Layout

```text
crates/crossover-core/   Deterministic event, policy, replay, and feature logic
crates/crossover-cli/    Command-line entry point for replay and benchmarking
docs/                    Problem framing, architecture, and success criteria
examples/                Stand-alone replay fixtures
```

## Current Commands

```sh
cargo run -p crossover-cli -- replay examples/events.pipe
```

Replays fixture events through the deterministic pipeline and prints feature
updates, placement decisions, and latency summaries.

```sh
cargo run -p crossover-cli -- bench 100000
```

Generates synthetic tick events and measures deterministic pipeline throughput.

```sh
cargo run -p crossover-cli -- generate-scenario \
  examples/binance_btcusdt_aggtrades_2025-01-02_1k.pipe \
  data/generated/scenarios/binance_btcusdt_1k_messy.pipe \
  messy \
  1000 \
  42
```

Derives a deterministic real-ish scenario from an existing pipe fixture. The
rollup profiles are `ordered`, `messy`, and `adversarial`. Targeted failure-mode
profiles include `capacity_exhaustion`, `out_of_order_burst`, `sequence_gap`,
`late_arrival`, `correction`, `clock_skew`, and `stale_source`.

## What This Is Not

Crossover is not:

- a trading strategy
- a stock predictor
- a portfolio optimizer
- a live market data connector
- a wrapper around an LLM

It is a systems artifact for protecting a low-latency financial event path while
admitting useful background work with clear evidence.
