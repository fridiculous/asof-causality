.PHONY: check test replay check-fixture bench

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

replay:
	cargo run -p asof-causality-cli -- replay examples/late-arrival.pipe

check-fixture:
	cargo run -p asof-causality-cli -- check examples/late-arrival.pipe

bench:
	cargo run -p asof-causality-cli -- bench --events 1000000 --symbols 1024
