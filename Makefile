.PHONY: check test replay check-fixture run-suite-late-heavy verify-real-data-demo verify-real-revision-demo bench

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

test:
	cargo test --workspace

replay:
	cargo run -p asof-causality-cli -- replay examples/late-arrival.pipe

check-fixture:
	cargo run -p asof-causality-cli -- check examples/late-arrival.pipe

run-suite-late-heavy:
	cargo run -p asof-causality-cli -- run-suite --scenario late-heavy --events 100000 --symbols 1024 --seed 42 --out runs/late-heavy

verify-real-data-demo:
	uv run --script scripts/rebuild-alfred-example.py --check

verify-real-revision-demo:
	uv run --script scripts/rebuild-alfred-revision-example.py --check
	uv run --script scripts/rebuild-alfred-revision-example.py --variant large --check

bench:
	cargo run -p asof-causality-cli -- bench --events 1000000 --symbols 1024
