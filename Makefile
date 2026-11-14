# Contract -> code (ADR 0055). The files in src/generated/ are never edited by hand.
INFI_BACKEND ?= ../infi/backend

.PHONY: generate check-generated sync-contract fmt clippy test build examples check

generate:
	cargo xtask generate

# Fails if the generated code does not match api/openapi.sdk.json.
check-generated: generate
	git diff --exit-code -- src/generated
	@test -z "$$(git ls-files --others --exclude-standard -- src/generated)" || (echo "generated file outside git"; exit 1)

# Copies the contract from the monorepo and regenerates.
sync-contract:
	cp $(INFI_BACKEND)/api/openapi.sdk.json $(INFI_BACKEND)/api/openapi.sdk.yaml api/
	$(MAKE) generate

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

build:
	cargo build --workspace --all-targets

# Examples against the real sandbox (needs INFI_SECRET_KEY).
examples:
	cargo run --example run-all

check: check-generated
	cargo fmt --all --check
	$(MAKE) clippy test build
