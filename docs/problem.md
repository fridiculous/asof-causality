# Problem

Backtests often fail before signal quality is even relevant: they accidentally
allow future data to influence past prediction records.

The common leak is subtle. A row may have an economic `observed_time`, but the
research system only learned about it at a later `received_time`. If a backtest
sorts by observed time, rewrites old rows, or gives the signal function access
to the full dataset, it can produce predictions that could not have existed in
the real world.

asof-causality treats "what was knowable when" as the central correctness
boundary. For institutional research teams, analytics vendors, and quant
reviewers who need to defend historical claims, methodology transparency has to
be executable. Every `PredictionRecord` carries the input event IDs its
`SignalEvaluation` used and the maximum replay key of those inputs. That turns
"could this result have known what it claims to know?" into a checkable
invariant rather than a README assertion. The system evaluates causality, not
strategy performance.

## Goal

Implement a point-in-time causality test suite that:

- processes events by `(received_time, received_sequence_number, event_id)`
- appends immutable `PredictionRecord`s
- restricts signal code to as-of state
- proves future rows cannot affect prior `PredictionRecord`s

The built-in signals in `asof-signals` keep alpha claims out of scope, but the
cage is exercised with recognizable shapes: fixed-point numeric windows and a
volatility-adjusted momentum crossover.

## Related Systems

General backtesters are usually optimized around portfolio simulation, order
modeling, and research ergonomics. This artifact is intentionally narrower: it
is a reference replay engine plus adversarial test suite for receipt-time
causality. If a production research platform already tracks receipt-time
semantics internally, the `asof` CLI is the executable fixture and negative
control that make those semantics independently inspectable.
