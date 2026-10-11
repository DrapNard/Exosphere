.PHONY: fmt check test lint ci
fmt:
	cargo fmt --all
check:
	cargo check --workspace --all-targets
test:
	cargo test --workspace
lint:
	cargo clippy --workspace --all-targets -- -D warnings
ci: fmt check lint test
