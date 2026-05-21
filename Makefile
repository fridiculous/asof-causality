.PHONY: ci check test doc build release-smoke package replay check-fixture run-suite-late-heavy verify-real-data-demo verify-real-revision-demo bench

ci: check test doc build release-smoke package

check:
	cargo fmt --check
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace --locked

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked

build:
	cargo build --release --locked

release-smoke: build
	target/release/asof-causality replay examples/late-arrival.pipe
	target/release/asof-causality check examples/late-arrival.pipe
	target/release/asof-causality negative-control examples/lookahead-negative-control.pipe

package:
	cargo package --locked --allow-dirty -p asof-causality-core

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
