# AI And GPU Sidecars

A sidecar is a companion component that runs beside the main system without
blocking the critical path.

In Crossover, the hot path stays deterministic:

```text
event arrives -> parse -> normalize -> compute essential features -> route alert
```

Sidecars receive a copy of the event stream and enrich it later:

```text
event copy -> GPU batch worker or AI worker -> enriched output
```

## AI Sidecar

An AI sidecar is an asynchronous worker that uses an LLM or model for fuzzy,
human-facing tasks:

- classify a news event
- explain why an alert fired
- summarize a filing
- repair malformed text
- generate research hypotheses

If the LLM is slow, fails, gets rate-limited, or returns nondeterministic output,
the real-time pipeline keeps running.

Example:

```text
CFO resignation event arrives

Hot path, about 1ms:
- parse event
- tag symbol
- trigger governance alert

AI sidecar, seconds later:
- summarize event
- explain possible market relevance
- attach caveats
```

## GPU Sidecar

A GPU sidecar is an asynchronous worker that receives micro-batches of events or
features and runs high-throughput computation on the GPU.

Good GPU-sidecar tasks include:

- feature matrices
- parameter sweeps
- cross-sectional ranks
- Monte Carlo
- batch anomaly detection

Weak GPU-sidecar tasks include:

- every single tick one-by-one
- tiny urgent decisions where transfer and kernel-launch overhead dominate
- blocking control-flow decisions in the ingest path

Example:

```text
Hot path:
- update latest price for each symbol

GPU sidecar every 5ms:
- compute 500 symbols x 50 features
- return anomaly scores
```

## Core Rule

The hot path stays deterministic and low-latency. AI and GPU sidecars add
intelligence or throughput without becoming real-time dependencies.

That is the discipline this project is meant to test: useful AI/GPU work is
allowed, but it must be isolated from the part of the system that cannot afford
surprises.
