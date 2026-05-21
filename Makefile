.PHONY: check test replay check-fixture verify-real-data-demo bench

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

replay:
	cargo run -p asof-causality-cli -- replay examples/late-arrival.pipe

check-fixture:
	cargo run -p asof-causality-cli -- check examples/late-arrival.pipe

verify-real-data-demo:
	python3 scripts/rebuild-alfred-demo.py --check

bench:
	cargo run -p asof-causality-cli -- bench --events 1000000 --symbols 1024
