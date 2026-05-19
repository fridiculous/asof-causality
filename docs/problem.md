# Problem

Backtests often fail before strategy quality is even relevant: they accidentally
allow future data to influence past predictions.

The common leak is subtle. A row may have an economic `observed_time`, but the
research system only learned about it at a later `received_time`. If a backtest
sorts by observed time, rewrites old rows, or gives the signal function access
to the full dataset, it can produce predictions that could not have existed in
the real world.

asof-replay treats "what was knowable when" as the central correctness boundary.

## Goal

Implement a point-in-time replay engine that:

- processes events by `(received_time, sequence)`
- emits immutable predictions
- restricts signal code to as-of state
- proves future rows cannot affect past predictions

The signal is deliberately simple so the correctness properties remain the
center of the artifact.
