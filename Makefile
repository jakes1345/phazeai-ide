.PHONY: build check test clean release cli ide fmt clippy install

# Default: build everything in debug mode
build:
	cargo build --workspace

# Check compilation without building
check:
	cargo check --workspace

# Run all tests
test:
	cargo test --workspace

# Format code
fmt:
	cargo fmt --all

# Lint with clippy
clippy:
	cargo clippy --workspace -- -D warnings

# Build release binaries
release:
	cargo build --release --workspace

# Build only the CLI
cli:
	cargo build -p phazeai-cli

# Build only the IDE
ide:
	cargo build -p phazeai-ide

# Install CLI to ~/.cargo/bin
install:
	cargo install --path crates/phazeai-cli

# Install IDE to ~/.cargo/bin
install-ide:
	cargo install --path crates/phazeai-ide

# Clean build artifacts
clean:
	cargo clean

# Run the CLI in debug mode
run-cli:
	cargo run -p phazeai-cli

# Run the IDE in debug mode
run-ide:
	cargo run -p phazeai-ide

# Cross-compile for a specific target
# Usage: make cross TARGET=x86_64-pc-windows-gnu
cross:
	cargo build --release --target $(TARGET) --workspace

# Package release binaries into dist/
dist: release
	mkdir -p dist
	cp target/release/phazeai dist/ 2>/dev/null || true
	cp target/release/phazeai.exe dist/ 2>/dev/null || true
	cp target/release/phazeai-ide dist/ 2>/dev/null || true
	cp target/release/phazeai-ide.exe dist/ 2>/dev/null || true
	@echo "Release binaries in dist/"
