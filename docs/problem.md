# Problem

Backtests often fail before signal quality is even relevant: they accidentally
allow future data to influence past predictions.

The common leak is subtle. A row may have an economic `observed_time`, but the
research system only learned about it at a later `received_time`. If a backtest
sorts by observed time, rewrites old rows, or gives the signal function access
to the full dataset, it can produce predictions that could not have existed in
the real world.

asof-causality treats "what was knowable when" as the central correctness
boundary. For institutional research teams, analytics vendors, and quant
reviewers who need to defend historical claims, methodology transparency has to
be executable. Every prediction carries the input event IDs it used and the
maximum replay key of those inputs. That turns "could this result have known
what it claims to know?" into a checkable invariant rather than a README
assertion. The system evaluates causality, not strategy performance.

## Goal

Implement a point-in-time causality test suite that:

- processes events by `(received_time, sequence, event_id)`
- emits immutable predictions
- restricts signal code to as-of state
- proves future rows cannot affect past predictions

The built-in signals keep alpha claims out of scope, but the cage is exercised
with recognizable residents: a fixed-point volatility-adjusted momentum signal
with a fast/slow moving-average crossover, plus fixed-point numeric window
statistics. That keeps the focus on causality while still demonstrating that
the boundary holds familiar signal shapes.

## Related Systems

General backtesters are usually optimized around portfolio simulation, order
modeling, and research ergonomics. This artifact is intentionally narrower: it
is a reference replay engine plus adversarial test suite for receipt-time
causality. If a production research platform already tracks receipt-time
semantics internally, `asof-causality` is the executable fixture and negative
control that make those semantics independently inspectable.
