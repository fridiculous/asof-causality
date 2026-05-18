# Problem

Modern financial data systems are under pressure to enrich every event:
classify news, summarize filings, explain alerts, repair messy records, compute
large feature batches, and route human attention.

The failure mode is simple: slow or unreliable work creeps into the path that
must stay fast. Once that happens, real-time ingest becomes harder to explain,
harder to replay, and easier to break under load.

Crossover treats this as a decision integrity problem.

## Research Question

When a financial event arrives, what work should:

- run immediately on the deterministic fast path
- run later on a GPU-shaped background worker
- run later on an LLM-shaped background worker
- be marked offline, deferred, or dropped

## Design Principle

The fast path must stay:

- deterministic
- replayable
- measurable
- protected from optional dependency failure

Optional enrichment is allowed only when the system can explain why the work is
worth doing and why it will not violate latency, deadline, or capacity
constraints.
