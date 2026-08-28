PNPM ?= pnpm
CARGO ?= cargo
TARGET_ARG := $(if $(TARGET),--target $(TARGET),)
CONFIG_ARG := $(if $(CONFIG),--config $(CONFIG),)
LINUX_PROFILE ?= release
WINDOWS_PROFILE ?= release

.DEFAULT_GOAL := help

.PHONY: help setup format format-check lint test test-rust test-web web check ci dev app dmg bundles linux smoke-linux windows smoke-windows

help:
	@echo "CleanerX development commands"
	@echo "  make setup             Install locked frontend dependencies"
	@echo "  make dev               Run the Tauri development app"
	@echo "  make format            Format Rust sources"
	@echo "  make check             Run formatting, lint, tests, and frontend build"
	@echo "  make app               Build an unsigned macOS .app"
	@echo "  make dmg               Build an unsigned macOS DMG"
	@echo "  make bundles           Build both .app and DMG in one Tauri run"
	@echo "  make linux             Build unsigned Linux .deb and AppImage bundles"
	@echo "  make smoke-linux       Launch the Linux binary under Xvfb for 8 seconds"
	@echo "  make windows           Build unsigned Windows MSI and NSIS installers"
	@echo "  make smoke-windows     Launch the Windows binary for 8 seconds"
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
	$(PNPM) tauri build $(CONFIG_ARG) $(TARGET_ARG) --bundles app

dmg:
	$(PNPM) tauri build $(CONFIG_ARG) $(TARGET_ARG) --bundles dmg

bundles:
	$(PNPM) tauri build $(CONFIG_ARG) $(TARGET_ARG) --bundles app,dmg

linux:
	$(PNPM) tauri build $(CONFIG_ARG) $(TARGET_ARG) --bundles deb,appimage

smoke-linux:
	@test "$$(uname -s)" = "Linux" || { echo "smoke-linux requires Linux" >&2; exit 2; }
	@test -x target/$(LINUX_PROFILE)/cleanerx-app || { echo "missing target/$(LINUX_PROFILE)/cleanerx-app" >&2; exit 2; }
	@mkdir -p target
	@status=0; timeout --signal=TERM 8s xvfb-run -a target/$(LINUX_PROFILE)/cleanerx-app >target/linux-smoke.log 2>&1 || status=$$?; \
		if test $$status -ne 124; then cat target/linux-smoke.log >&2; exit $$status; fi

windows:
	$(PNPM) tauri build $(CONFIG_ARG) $(TARGET_ARG) --bundles nsis,msi

smoke-windows:
	powershell -NoProfile -ExecutionPolicy Bypass -File scripts/smoke-windows.ps1 -Profile $(WINDOWS_PROFILE)
