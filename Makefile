.PHONY: check test replay check-fixture bench

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

replay:
	cargo run -p asof-replay-cli -- replay examples/late-arrival.pipe

check-fixture:
	cargo run -p asof-replay-cli -- check examples/late-arrival.pipe

bench:
	cargo run -p asof-replay-cli -- bench --events 1000000 --symbols 1024
