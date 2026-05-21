use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub struct BenchResult {
    pub representation: &'static str,
    pub events: usize,
    pub symbols: usize,
    pub elapsed_ns: u128,
    pub events_per_second: f64,
    pub checksum: i64,
}

pub fn run_representation_benchmark(events: usize, symbols: usize) -> Vec<BenchResult> {
    vec![
        bench_string_map(events, symbols.max(1)),
        bench_symbol_slot_vec(events, symbols.max(1)),
    ]
}

fn bench_string_map(events: usize, symbols: usize) -> BenchResult {
    let symbol_names: Vec<String> = (0..symbols).map(|index| format!("SYM{index}")).collect();
    let mut state: BTreeMap<String, i8> = symbol_names
        .iter()
        .map(|symbol| (symbol.clone(), 0))
        .collect();
    let mut checksum = 0_i64;
    let start = Instant::now();

    for index in 0..events {
        let symbol = &symbol_names[index % symbols];
        let value = sentiment_value(index);
        if let Some(slot) = state.get_mut(symbol) {
            *slot = value;
            checksum += i64::from(*slot);
        }
    }

    black_box(checksum);
    result(
        "string map",
        events,
        symbols,
        start.elapsed().as_nanos(),
        checksum,
    )
}

fn bench_symbol_slot_vec(events: usize, symbols: usize) -> BenchResult {
    let mut state = vec![0_i8; symbols];
    let mut checksum = 0_i64;
    let start = Instant::now();

    for index in 0..events {
        let symbol_slot = index % symbols;
        state[symbol_slot] = sentiment_value(index);
        checksum += i64::from(state[symbol_slot]);
    }

    black_box(checksum);
    result(
        "symbol slot vec",
        events,
        symbols,
        start.elapsed().as_nanos(),
        checksum,
    )
}

fn sentiment_value(index: usize) -> i8 {
    match index % 3 {
        0 => -1,
        1 => 0,
        _ => 1,
    }
}

fn result(
    representation: &'static str,
    events: usize,
    symbols: usize,
    elapsed_ns: u128,
    checksum: i64,
) -> BenchResult {
    let seconds = elapsed_ns as f64 / 1_000_000_000.0;
    let events_per_second = if seconds > 0.0 {
        events as f64 / seconds
    } else {
        0.0
    };

    BenchResult {
        representation,
        events,
        symbols,
        elapsed_ns,
        events_per_second,
        checksum,
    }
}
