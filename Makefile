PNPM ?= pnpm
CARGO ?= cargo
TARGET_ARG := $(if $(TARGET),--target $(TARGET),)

.DEFAULT_GOAL := help

.PHONY: help setup format format-check lint test test-rust test-web web check ci dev app dmg bundles

help:
	@echo "CleanerX development commands"
	@echo "  make setup             Install locked frontend dependencies"
	@echo "  make dev               Run the Tauri development app"
	@echo "  make format            Format Rust sources"
	@echo "  make check             Run formatting, lint, tests, and frontend build"
	@echo "  make app               Build an unsigned macOS .app"
	@echo "  make dmg               Build an unsigned macOS DMG"
	@echo "  make bundles           Build both .app and DMG in one Tauri run"
	@echo "  make app TARGET=...    Build for an explicit Rust target triple"

setup:
	$(PNPM) install --frozen-lockfile

format:
	$(CARGO) fmt --all

format-check:
	$(CARGO) fmt --all -- --check

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

test-rust:
	$(CARGO) test --workspace

test-web:
	$(PNPM) test

test: test-rust test-web

web:
	$(PNPM) build

check: format-check lint test web

ci: check

dev:
	$(PNPM) tauri dev

app:
	$(PNPM) tauri build $(TARGET_ARG) --bundles app

dmg:
	$(PNPM) tauri build $(TARGET_ARG) --bundles dmg

bundles:
	$(PNPM) tauri build $(TARGET_ARG) --bundles app,dmg
