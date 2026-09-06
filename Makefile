.PHONY: build test lint check package clean

# Build the release cdylib
build:
	cargo build --release

# Run unit tests
test:
	cargo nextest run --all-features --all-targets

# Lint (levels governed by Cargo.toml [lints.clippy] cherry-pick)
lint:
	cargo clippy --all-features --all-targets

# Full local CI gate (clippy + test + build + deny + gitleaks)
check:
	./scripts/check-local.sh

# Package as .igplugin.zip
package:
	./scripts/package.sh

clean:
	cargo clean