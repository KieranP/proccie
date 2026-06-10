.DEFAULT_GOAL := help

.PHONY: build test fmt fmt-check clippy check clean install help

build: ## Build the release binary to target/release/proccie
	cargo build --release

test: ## Run all tests
	cargo test

fmt: ## Format the code
	cargo fmt

fmt-check: ## Check formatting without modifying files
	cargo fmt --check

clippy: ## Run clippy (lints as errors)
	cargo clippy --all-targets -- -D warnings

check: fmt-check clippy test ## Run formatting check, clippy, and tests

clean: ## Remove build artifacts
	cargo clean

install: ## Install proccie to ~/.cargo/bin
	cargo install --path .

help: ## Show this help
	@grep -E '^[a-z][a-zA-Z0-9_-]+:.*##' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'
