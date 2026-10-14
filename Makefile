# Always build for SBPF v0: it deploys on every cluster and runs in LiteSVM's
# default runtime. (Anchor 1.x defaults to v3, which LiteSVM rejects.)
ARCH ?= v0

.PHONY: build test lint fmt

# The example program the integration tests run against.
build:
	cargo build-sbf --manifest-path examples/launchpad/Cargo.toml --arch $(ARCH) --sbf-out-dir target/deploy

test: build
	cargo test --workspace
	cargo test -p anchor-raydium-cpmm --features devnet

lint:
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all
