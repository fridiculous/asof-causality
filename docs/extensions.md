# Extensions

asof-replay's current implementation is intentionally small: Rust signals read
from `AsOfView`, and the built-in signal uses one per-symbol sentiment snapshot.
The extension path keeps the same rule: the replay engine owns time, and
strategy code only receives data that was knowable at the prediction time.

## Python Strategies

Python integration is not implemented in this version. The intended safe shape
is a subprocess or channel boundary:

```text
Rust replay engine
  -> orders events by received_time
  -> maintains StateStore
  -> sends one as-of snapshot to Python at each prediction
  -> receives one signal value back
  -> writes PredictionRecord with provenance
```

Python should not receive the full event file or a full-day dataframe. The cage
holds because the runner controls what crosses the wire: only the current
snapshot and its provenance. JSON Lines would be the simplest readable protocol;
MessagePack over a Unix domain socket would be a natural lower-overhead version
if the boundary became performance-sensitive.

## Growing `AsOfView`

The current public surface has one accessor:

```text
snapshot(symbol) -> SymbolSnapshot
```

Real strategies need more state. That growth should be additive:

```text
recent_ticks(symbol, window)
vwap_window(symbol, window)
latest_filing(symbol)
cross_symbol_snapshot(symbols)
```

Each accessor has the same obligation as `snapshot`: return only state derived
from received events and include provenance sufficient to compute
`max_input_replay_key`, including both received time and sequence. Adding an
accessor is therefore a small proof obligation, not a reason to expose the
underlying event list.

## Placement Policy

The earlier Crossover scaffold treated hot-path, sidecar, and offline work as a
placement problem. That policy is orthogonal to asof-replay's correctness
contract. A signal should first pass point-in-time replay; only then should its
runtime placement be chosen from measured latency and failure behavior.

In that framing, asof-replay is the offline correctness lane: it proves that a
strategy's historical predictions were possible before anyone decides whether
the strategy belongs in a hot path, a sidecar, or an offline research loop.
