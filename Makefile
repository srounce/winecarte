SHELL := /usr/bin/env bash

# ------------------------------------------------------------------------------
# configuration
# ------------------------------------------------------------------------------

# Host target triple from rustc.
HOST_TARGET := $(shell rustc -vV | sed -n 's/^host: //p')

# Host target triple from rustc.
HOST_ARCH := $(shell echo $(HOST_TARGET) | sed 's/\([^-]*\).*/\1/')

# Binary target names that should be built for Windows instead of the host.
WINDOWS_BINS := wine2linux

# Windows target triple for the binaries above.
WINDOWS_TARGET ?= $(HOST_ARCH)-pc-windows-gnu

# Workspace manifest path.
CARGO_MANIFEST ?= Cargo.toml

# ------------------------------------------------------------------------------
# binary discovery
# ------------------------------------------------------------------------------
#
# We use `cargo metadata --no-deps` and extract:
#   - package manifest path
#   - target kind
#   - target name
#
# Then we keep only:
#   - packages whose manifest lives under crates/
#   - targets whose kind is ["bin"]
#
# This avoids filesystem guessing and respects explicit [[bin]] entries.
#
# Note: this is a lightweight text extraction, not a general JSON parser.
# It is good enough for Cargo metadata's stable structure for this use case.

BINS := $(shell \
	cargo metadata --format-version 1 --no-deps --manifest-path $(CARGO_MANIFEST) \
	| tr '\n' ' ' \
	| sed 's/},{/},\n{/g' \
	| grep '"manifest_path":"[^"]*/crates/[^"]*/Cargo.toml"' \
	| grep '"kind":\["bin"\]' \
	| sed -n 's/.*"kind":\["bin"\].*"name":"\([^"]*\)".*/\1/p' \
	| sort -u \
)

# ------------------------------------------------------------------------------
# helpers
# ------------------------------------------------------------------------------

define is_windows_bin
$(filter $(1),$(WINDOWS_BINS))
endef

define cargo_build_bin
@if [ -n "$(call is_windows_bin,$(1))" ]; then \
	echo "==> building $(1) for $(WINDOWS_TARGET)"; \
	cargo build --manifest-path $(CARGO_MANIFEST) --bin $(1) --target $(WINDOWS_TARGET); \
else \
	echo "==> building $(1) for host ($(HOST_TARGET))"; \
	cargo build --manifest-path $(CARGO_MANIFEST) --bin $(1); \
fi
endef

# ------------------------------------------------------------------------------
# public targets
# ------------------------------------------------------------------------------

.PHONY: all list clean $(BINS)

all: $(BINS)

list:
	@printf '%s\n' $(BINS)

clean:
	cargo clean --manifest-path $(CARGO_MANIFEST)

$(foreach bin,$(BINS),$(eval \
$(bin): ; $$(call cargo_build_bin,$(bin)) \
))
