all: build test

build:
	@cargo build

check:
	@cargo check

doc:
	@cargo doc

test:
	@cargo test
	@cargo test --all-features

test-msrv:
	@cargo test
	@cargo test --features=slab

format:
	@rustup component add rustfmt 2> /dev/null
	@cargo fmt --all

format-check:
	@rustup component add rustfmt 2> /dev/null
	@cargo fmt --all -- --check

lint:
	@rustup component add clippy 2> /dev/null
	@cargo clippy

.PHONY: all check doc test test-msrv format format-check lint
