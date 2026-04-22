# Makefile — driver for the Go binding.
#
# Building the Go binding requires the lora-ffi Rust static library on
# disk. These targets build it (via `cargo` at the workspace root) and
# then run the Go toolchain against it. CI uses the same targets.

# Path to the workspace target/ directory — two levels up from the
# module directory (crates/lora-go → lora).
WORKSPACE_ROOT := $(abspath $(CURDIR)/../..)
CARGO_TARGET   := $(WORKSPACE_ROOT)/target/release

# Everything is built in release mode. Debug builds of the engine are
# ~10x slower and tests timeout on CI if they hit that path.

.PHONY: all build ffi go-vet go-test test fmt lint check example clean help

all: build go-vet go-test

help:
	@echo "targets:"
	@echo "  build     — build the Rust FFI static lib + cdylib (release)"
	@echo "  go-vet    — go vet ./..."
	@echo "  go-test   — go test -race ./..."
	@echo "  test      — build + go-vet + go-test"
	@echo "  fmt       — gofmt -w ."
	@echo "  lint      — gofmt -l . (non-zero exit on drift)"
	@echo "  example   — go run ./examples/basic"
	@echo "  clean     — go clean + drop target/release/liblora_ffi.*"

build: ffi

ffi:
	cd $(WORKSPACE_ROOT) && cargo build --release -p lora-ffi

go-vet:
	go vet ./...

go-test:
	go test -race ./...

test: build go-vet go-test

fmt:
	gofmt -w .

lint:
	@diff=$$(gofmt -l .); \
	if [ -n "$$diff" ]; then \
		echo "gofmt drift:"; \
		echo "$$diff"; \
		exit 1; \
	fi

check: lint go-vet go-test

example: build
	go run ./examples/basic

clean:
	go clean -testcache
	rm -f $(CARGO_TARGET)/liblora_ffi.a $(CARGO_TARGET)/liblora_ffi.dylib $(CARGO_TARGET)/liblora_ffi.so
