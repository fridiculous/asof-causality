# Threat Model

asof-replay is a correctness cage for one failure class: predictions that depend
on information the system had not received yet. It is not a general sandbox, a
data-vendor validator, or an alpha-quality test.

## Protected Boundary

The protected path is:

```text
received-time replay
  -> StateStore updated only by received events
  -> AsOfView exposed to signal code
  -> PredictionRecord with input provenance
  -> audit checks over max_input_replay_key
```

For Rust signals using the public `Signal` API, future rows are not reachable
through `AsOfView`: `StateStore` and `StateWriter` are crate-private, and the
view exposes only accessor methods that return provenance.

## Out Of Scope

- **Bad upstream timestamps.** The engine trusts `received_time`. If a vendor or
  ingestion system records the wrong receipt time, asof-replay will enforce the
  wrong contract consistently.
- **Missing events.** A signal can be honest and still bad if important events
  never arrive. That is a feed-completeness problem, not lookahead leakage.
- **External side channels.** A strategy that reads the filesystem, network,
  wall clock, environment variables, or another database can bypass the cage.
  Python subprocess support would need an explicit sandbox policy before making
  stronger claims.
- **Mutable signal memory.** The current built-in signal is stateless. A custom
  signal with interior mutability can carry information across prediction calls;
  that may be legitimate rolling state, but it is outside the current provenance
  model unless the state is derived only from `AsOfView` observations.
- **Future API accessors.** `AsOfView::snapshot` is the only accessor today.
  Future accessors such as `recent_ticks` or `vwap_window` must return enough
  provenance to compute `max_input_replay_key`, including sequence ties inside
  the same received timestamp.
- **Timing channels.** The tool does not treat runtime latency as secret. If a
  deployment considers timing behavior sensitive, that belongs in the runtime
  placement/sandbox layer, not this offline replay proof.

## Review Rule

Every new way for strategy code to learn information must answer:

```text
Can this return data whose (received_time, sequence) is greater than the
prediction key?
Can the PredictionRecord prove that it did not?
```

If either answer is unclear, the accessor or integration weakens the cage.
