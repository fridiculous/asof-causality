# Test Data

This directory is for optional market-data samples used during local testing.
Bulk downloads and generated files are ignored by git.

## Binance BTCUSDT aggTrades

Pulled sample:

- Source: Binance public data
- URL: `https://data.binance.vision/data/spot/daily/aggTrades/BTCUSDT/BTCUSDT-aggTrades-2025-01-02.zip`
- Checksum URL: `https://data.binance.vision/data/spot/daily/aggTrades/BTCUSDT/BTCUSDT-aggTrades-2025-01-02.zip.CHECKSUM`
- Raw compressed size: about 18 MB
- Raw CSV rows: 1,299,165

Local files:

- `data/external/binance/spot/daily/aggTrades/BTCUSDT/BTCUSDT-aggTrades-2025-01-02.zip`
- `data/generated/binance/BTCUSDT-aggTrades-2025-01-02_100k.pipe`
- `examples/binance_btcusdt_aggtrades_2025-01-02_1k.pipe`
- `data/generated/scenarios/*.pipe`

Conversion notes:

- Binance `timestamp` is treated as microseconds and converted to `observed_ns`
  by appending three zeroes.
- Binance `price` is converted to integer `price_cents`.
- Binance `quantity` is converted to integer lots using `quantity * 100_000_000`.
- Extra Binance fields remain in the payload for later experiments, while the
  current feature engine reads only `price_cents` and `size`.

## Scenario Generation

The CLI can derive repeatable test scenarios from any pipe fixture:

```sh
cargo run -p crossover-cli -- generate-scenario \
  examples/binance_btcusdt_aggtrades_2025-01-02_1k.pipe \
  data/generated/scenarios/binance_btcusdt_1k_messy.pipe \
  messy \
  1000 \
  42
```

Rollup profiles:

- `ordered`: mostly append-only ingest with small latency jitter.
- `messy`: realistic latency distribution plus duplicate events.
- `adversarial`: larger out-of-order latency, dropped events, duplicate events,
  source clock skew markers, and correction events.

Targeted failure-mode profiles:

- `capacity_exhaustion`: expands the seed stream to the requested event count
  and marks all events with `capacity_probe=true`.
- `out_of_order_burst`: injects a contiguous burst of high-latency events so
  ingest order diverges from source-time order.
- `sequence_gap`: introduces one explicit sequence jump marked with
  `sequence_gap_size`.
- `late_arrival`: delays selected old source-time events by 60-120 seconds and
  marks them with `late_arrival=true`.
- `correction`: appends correction events with the original source time and a
  later ingest time, marked with `correction_for`.
- `clock_skew`: emits future-dated source timestamps, marked with
  `clock_skew=forward_source_clock` and `source_ahead_ns`.
- `stale_source`: rotates simulated venues, drops one source during a window,
  and marks surviving records with `stale_source` and `quorum=partial`.

Generated scenario files use `observed_ns` as simulated ingest time. The source
timestamp is preserved in the payload as `source_time_ns`, with
`ingest_time_ns` and `latency_ns` added so future bitemporal tests can query
both time axes explicitly.

The generator can expand a small seed file by cycling it deterministically. For
example, this creates a 50,000-event capacity-pressure stream from the 1,000-row
Binance fixture:

```sh
cargo run -p crossover-cli -- generate-scenario \
  examples/binance_btcusdt_aggtrades_2025-01-02_1k.pipe \
  data/generated/scenarios/failure_capacity_exhaustion_50k.pipe \
  capacity_exhaustion \
  50000 \
  42
```
