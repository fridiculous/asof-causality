.PHONY: check test replay bench

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

replay:
	cargo run -p crossover-cli -- replay examples/events.pipe

bench:
	cargo run -p crossover-cli -- bench 100000
