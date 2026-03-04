.DEFAULT_GOAL := help

VERSION := $(shell git describe --tags --always --dirty 2>/dev/null || echo dev)
LDFLAGS  := -ldflags '-X main.version=$(VERSION)'

.PHONY: build test vet lint check clean install help

build: ## Build the binary to bin/proccie
	go build $(LDFLAGS) -o bin/proccie ./cmd/proccie

test: ## Run all tests
	go test ./...

vet: ## Run go vet
	go vet ./...

lint: ## Run golangci-lint
	golangci-lint run

check: vet lint test ## Run vet, lint, and tests

format: ## Format the code
	golangci-lint fmt

clean: ## Remove build artifacts
	rm -f bin/proccie

install: ## Install proccie to GOPATH/bin
	go install $(LDFLAGS) ./cmd/proccie

help: ## Show this help
	@grep -E '^[a-z][a-zA-Z0-9_-]+:.*##' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'
