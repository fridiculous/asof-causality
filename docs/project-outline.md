# Project Outline

## Summary

Crossover is a small system for financial event triage.

It answers:

> When a financial event arrives, what work should happen immediately, what work
> can happen later, and what should be skipped?

The useful output is an audit trail of event decisions. The systems claim is
that the fast path stays deterministic and low-latency even when slower
background workers fall behind.

## What The System Does

For each event, Crossover will:

1. Parse and normalize the event.
2. Compute simple deterministic features.
3. Decide which optional work is worth doing.
4. Emit an audit record explaining the decision.
5. Send optional work to background workers only if it is worth the cost.
6. Report latency, dropped work, missed deadlines, and retained value.

Example decisions:

```text
Tick update -> handle immediately
Important news -> send to LLM-style background worker
Large numeric batch -> send to GPU-style background worker
Low-value stale item -> drop or mark offline
```

## Main Components

- Fast path: code that must run immediately for every event.
- Admission controller: decides whether optional work is worth doing now.
- Background workers: separate workers that imitate slow GPU/LLM work.
- Logger worker: writes output records so the fast path does not write files.
- Decision records: append-only records showing what happened and why.
- Comparison runner: compares simple routing against cost-benefit routing.

## Important Design Rules

- The fast path must not call an LLM.
- The fast path must not call a GPU.
- The fast path must not write to disk or stdout per event.
- The fast path must not wait for a background worker.
- Queues must have fixed capacity.
- If a queue is full, the system drops, defers, or marks work offline.
- Later background results are written as separate records linked by event id.
- Replay and benchmark results are separate:
  - replay hashes should be stable
  - benchmark timings may vary by machine

## Admission Logic

The controller first checks hard rules:

```text
Will this block the fast path? Reject.
Will it miss its deadline? Drop, defer, or mark offline.
Is the queue full? Drop, defer, or mark offline.
Is the work nondeterministic? Do not run it on the fast path.
```

If several safe choices remain, choose the one that keeps the most useful work:

```text
prefer urgent work
prefer fresh work
prefer high-value work
prefer work likely to finish before its deadline
prefer less overloaded workers
```

## Policies To Compare

- Static: fixed rules, such as "news always goes to the LLM worker."
- Greedy: use a worker whenever its queue has room.
- Cost-benefit: use a worker only when the work is valuable enough and likely
  to finish in time.

The point is to show that cost-benefit routing keeps more important work while
still protecting the fast path.

## Outputs

The CLI should produce:

- readable terminal summaries
- JSONL decision records
- comparison reports across policies
- stable replay hashes in replay mode

Each decision record should show:

```text
event id
symbol
event type
features
work considered
decision made
reason
deadline
whether background work completed
value retained or lost
```

## Future CLI Commands

```sh
crossover replay examples/events.pipe --policy cost-benefit --scenario steady --packets out.jsonl
crossover bench 100000 --mixed --policy cost-benefit --scenario llm-slow
crossover compare 100000 --scenario mixed-overload --out reports/
```

The current baseline supports:

```sh
cargo run -p crossover-cli -- replay examples/events.pipe
cargo run -p crossover-cli -- bench 100000
cargo test
```

## Test Plan

Future tests should prove:

- same replay input produces the same decisions
- the fast path does not block on background workers
- full queues cause drops, defers, or offline outcomes instead of waiting
- LLM-style work never runs on the fast path
- important work is kept before routine work under overload
- background results are separate records linked by event id
- cost-benefit beats static and greedy policies on retained urgent value in
  overload scenarios

## Success Criteria

The project succeeds if it shows:

- fast-path p99 latency stays under budget in benchmark mode
- replay mode produces stable hashes
- background workers can slow down or fail without stopping event processing
- queues stay bounded
- every decision has a clear reason
- cost-benefit keeps more urgent/high-value work than simpler policies

## Assumptions

- The project is event triage, not trading automation.
- The background GPU/LLM workers are realistic stand-ins, not real integrations.
- Real worker threads and real queues should be included in the next major step.
- The useful output is a clear audit trail of event decisions.
