.DEFAULT_GOAL := help

.PHONY: build test fmt fmt-check lint audit check clean install install-hooks help

build: ## Build the release binary to target/release/proccie
	cargo build --release

test: ## Run all tests
	cargo test

fmt: ## Format the code
	cargo fmt

fmt-check: ## Check formatting without modifying files
	cargo fmt --check

lint: ## Run clippy (lints as errors)
	cargo clippy --all-targets -- -D warnings

audit: ## Scan dependencies for security advisories (needs cargo-audit)
	cargo audit

check: fmt-check lint test ## Run formatting check, clippy, and tests

clean: ## Remove build artifacts
	cargo clean

install: ## Install proccie to ~/.cargo/bin
	cargo install --path .

install-hooks: ## Enable the pre-commit hook (runs make check)
	git config core.hooksPath .githooks

help: ## Show this help
	@grep -E '^[a-z][a-zA-Z0-9_-]+:.*##' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'
