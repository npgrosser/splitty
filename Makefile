FRONTEND_SRC := $(shell find frontend/src frontend/public -type f 2>/dev/null) \
                frontend/index.html frontend/package.json frontend/vite.config.ts
UNAME   := $(shell uname -s)
BUILD   := build
COMMIT  := $(shell git rev-parse --short HEAD 2>/dev/null || echo unknown)
# The workspace version, which scripts/release.sh sets.
VERSION := $(shell sed -n '/^\[workspace\.package\]/,/^\[/ s/^version = "\(.*\)"$$/\1/p' Cargo.toml)

FRONTEND_STAMP := frontend/.build-stamp

.PHONY: dev dev-tauri clean test icons electron dmg \
       install-no-gui install-tauri install-electron \
       uninstall-no-gui uninstall-tauri uninstall-electron uninstall-config \
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
	cargo watch -i frontend/ -i frontend-electron/ -x 'run -p splitty-server -- serve'

dev-tauri: frontend/node_modules
	@cd frontend && node_modules/.bin/vite --port 1420 --strictPort & VITE_PID=$$!; \
	trap "kill $$VITE_PID 2>/dev/null; wait $$VITE_PID 2>/dev/null" EXIT INT TERM; \
	sleep 1; \
	cd crates/splitty-tauri && cargo tauri dev

# --- Test ---

test:
	cargo test

# --- macOS app bundle ---

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

uninstall-electron:
	rm -f $(HOME)/.local/bin/Splitty-Electron.AppImage
	rm -f $(HOME)/.local/share/applications/splitty.desktop
	rm -f $(HOME)/.local/share/icons/hicolor/128x128/apps/splitty.png
	@echo "Removed Splitty (Electron)"

uninstall-config:
	rm -rf $(HOME)/.config/splitty
	@echo "Removed $(HOME)/.config/splitty"

# --- Electron ---

frontend-electron/node_modules: frontend-electron/package.json
	cd frontend-electron && pnpm install
	@touch $@

electron: $(BUILD)/splitty frontend-electron/node_modules
	cd frontend-electron && pnpm run dist

install-electron: electron
	mkdir -p $(HOME)/.local/bin $(HOME)/.local/share/applications $(HOME)/.local/share/icons/hicolor/128x128/apps
	cp frontend-electron/dist/Splitty-$(VERSION).AppImage $(HOME)/.local/bin/Splitty-Electron.AppImage
	chmod +x $(HOME)/.local/bin/Splitty-Electron.AppImage
	cp packaging/linux/splitty.png $(HOME)/.local/share/icons/hicolor/128x128/apps/splitty.png
	sed 's|Splitty-Electron.AppImage|$(HOME)/.local/bin/Splitty-Electron.AppImage|g' \
		packaging/electron/linux/splitty.desktop > $(HOME)/.local/share/applications/splitty.desktop
	@echo "Installed to $(HOME)/.local/bin/Splitty-Electron.AppImage + desktop entry"

run-electron: $(BUILD)/splitty frontend-electron/node_modules
	cd frontend-electron && pnpm run dev

# --- Icons ---

icons:
	./scripts/generate-icons.sh

# --- Clean ---

clean:
	rm -rf $(BUILD) $(FRONTEND_STAMP)
	rm -rf frontend/dist
	cargo clean
