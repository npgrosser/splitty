FRONTEND_SRC := $(shell find frontend/src frontend/public -type f 2>/dev/null) \
                frontend/index.html frontend/package.json frontend/vite.config.ts
UNAME   := $(shell uname -s)
BUILD   := build
COMMIT  := $(shell git rev-parse --short HEAD 2>/dev/null || echo unknown)

FRONTEND_STAMP := frontend/.build-stamp

.PHONY: dev dev-tauri clean test icons dmg native-package \
       install-no-gui install-tauri \
       uninstall-no-gui uninstall-tauri uninstall-config \
       check-splitty-not-running

# --- Frontend ---

frontend/node_modules: frontend/package.json frontend/pnpm-lock.yaml
	cd frontend && pnpm install
	@touch $@

$(FRONTEND_STAMP): frontend/node_modules $(FRONTEND_SRC)
	cd frontend && pnpm run build
	@touch $@

# --- Build targets ---

$(BUILD):
	mkdir -p $@

$(BUILD)/splitty: $(FRONTEND_STAMP) | $(BUILD)
	SPLITTY_COMMIT=$(COMMIT) SPLITTY_BUILD_TIME=$$(date -u +%Y-%m-%dT%H:%M:%SZ) \
		cargo build --release -p splitty-server
	cp target/release/splitty $@

$(BUILD)/splitty-gui: $(FRONTEND_STAMP) | $(BUILD)
	SPLITTY_COMMIT=$(COMMIT) SPLITTY_BUILD_TIME=$$(date -u +%Y-%m-%dT%H:%M:%SZ) \
		cargo build --release -p splitty-gui
	cp target/release/splitty-gui $@

# --- Run targets ---

dev:
	cargo watch -i frontend/ -x 'run -p splitty-server -- serve'

dev-tauri: frontend/node_modules
	cd frontend && pnpm build
	@(cd frontend && exec node_modules/.bin/vite build --watch) & FRONTEND_WATCH_PID=$$!; \
	trap "kill $$FRONTEND_WATCH_PID 2>/dev/null; wait $$FRONTEND_WATCH_PID 2>/dev/null" EXIT INT TERM; \
	cd crates/splitty-tauri && cargo tauri dev

# --- Test ---

test:
	cargo test

# --- Native desktop app (crates/splitty-gpui) ---

# The packages a release publishes for this system, into build/: deb, rpm and
# tar.gz on Linux, a disk image on macOS. docs/development.md has Windows.
native-package:
ifeq ($(UNAME),Darwin)
	packaging/macos/native/build.sh
else
	packaging/linux/native/build.sh
endif

# --- Legacy web app: macOS app bundle (Tauri) ---

check-splitty-not-running:
ifeq ($(UNAME),Darwin)
	@if pgrep -x splitty-gui >/dev/null 2>&1; then \
		echo "Error: Splitty is running. Quit it with Cmd+Q, then try again."; \
		exit 1; \
	fi
endif

# Both carry the servers the app installs on remote hosts, built from this
# checkout (packaging/macos/build.sh).
$(BUILD)/Splitty.app: check-splitty-not-running | $(BUILD)
	packaging/macos/build.sh --skip-tests --bundles app

dmg:
	packaging/macos/build.sh

# --- Install ---

install-no-gui: $(BUILD)/splitty
	mkdir -p $(HOME)/.local/bin
	cp $(BUILD)/splitty $(HOME)/.local/bin/splitty
	@echo "Installed to $(HOME)/.local/bin/splitty"

ifeq ($(UNAME),Darwin)
install-tauri: $(BUILD)/splitty $(BUILD)/Splitty.app
	@if pgrep -x splitty-gui >/dev/null 2>&1; then \
		echo "Error: Splitty is running. Quit it with Cmd+Q, then try again."; \
		exit 1; \
	fi
	@set -eu; \
		staging_dir=$$(mktemp -d /Applications/.splitty-install.XXXXXX); \
		trap 'rm -rf "$$staging_dir"' 0; \
		ditto target/release/bundle/macos/Splitty.app "$$staging_dir/Splitty.app"; \
		if pgrep -x splitty-gui >/dev/null 2>&1; then \
			echo "Error: Splitty started during installation. Quit it, then try again."; \
			exit 1; \
		fi; \
		rm -rf /Applications/Splitty.app; \
		mv "$$staging_dir/Splitty.app" /Applications/Splitty.app; \
		rmdir "$$staging_dir"; \
		trap - 0
	@echo "Installed to /Applications/Splitty.app"
else
install-tauri: $(BUILD)/splitty $(BUILD)/splitty-gui
	mkdir -p $(HOME)/.local/bin $(HOME)/.local/share/applications $(HOME)/.local/share/icons/hicolor/128x128/apps
	cp $(BUILD)/splitty-gui $(HOME)/.local/bin/splitty-gui
	cp $(BUILD)/splitty $(HOME)/.local/bin/splitty
	cp packaging/linux/splitty.desktop $(HOME)/.local/share/applications/splitty.desktop
	cp packaging/linux/splitty.png $(HOME)/.local/share/icons/hicolor/128x128/apps/splitty.png
	@echo "Installed to $(HOME)/.local/bin/splitty-gui + splitty + desktop entry"
endif

# --- Uninstall ---

uninstall-no-gui:
	rm -f $(HOME)/.local/bin/splitty
	@echo "Removed $(HOME)/.local/bin/splitty"

uninstall-tauri:
ifeq ($(UNAME),Darwin)
	rm -rf /Applications/Splitty.app
	@echo "Removed /Applications/Splitty.app"
else
	rm -f $(HOME)/.local/bin/splitty-gui
	rm -f $(HOME)/.local/bin/splitty
	rm -f $(HOME)/.local/share/applications/splitty.desktop
	rm -f $(HOME)/.local/share/icons/hicolor/128x128/apps/splitty.png
	@echo "Removed Splitty (Tauri)"
endif

uninstall-config:
	rm -rf $(HOME)/.config/splitty
	@echo "Removed $(HOME)/.config/splitty"

# --- Icons ---

icons:
	./scripts/generate-icons.sh

# --- Clean ---

clean:
	rm -rf $(BUILD) $(FRONTEND_STAMP)
	rm -rf frontend/dist
	cargo clean
