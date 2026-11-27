UNAME   := $(shell uname -s)
BUILD   := build
COMMIT  := $(shell git rev-parse --short HEAD 2>/dev/null || echo unknown)

# Where `make install` puts the desktop app; DESTDIR stages it for a package.
PREFIX  ?= $(HOME)/.local
DESTDIR ?=
# The directory with the built splitty-gpui and splitty.
BIN_DIR ?= target/release

.PHONY: clean test icons native-package build install uninstall \
       install-no-gui uninstall-no-gui uninstall-config

# --- Build targets ---

$(BUILD)/splitty:
	SPLITTY_COMMIT=$(COMMIT) SPLITTY_BUILD_TIME=$$(date -u +%Y-%m-%dT%H:%M:%SZ) \
		cargo build --release -p splitty-server
	mkdir -p $(BUILD)
	cp target/release/splitty $@

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

# The app and the `splitty` command, into target/release. The pinned terminal
# library is built first unless GHOSTTY_VT_DIR names one.
build:
	GHOSTTY_VT_DIR=$${GHOSTTY_VT_DIR:-$$(python3 scripts/bootstrap-libghostty-vt.py)} \
	SPLITTY_COMMIT=$(COMMIT) SPLITTY_BUILD_TIME=$$(date -u +%Y-%m-%dT%H:%M:%SZ) \
		cargo build --locked --release -p splitty-gpui -p splitty-server

# --- Install ---

# The built app under PREFIX: the executable, the `splitty` command and
# shell-integration/ together in lib/splitty (the app looks for the other two
# beside itself), links in bin, the desktop entry and the icon. It installs
# what `make build` built and compiles nothing itself.
APP_DIR := $(DESTDIR)$(PREFIX)/lib/splitty
SHARE   := $(DESTDIR)$(PREFIX)/share

install:
	@for file in splitty-gpui splitty; do \
		[ -x "$(BIN_DIR)/$$file" ] || { echo "missing $(BIN_DIR)/$$file; run 'make build' first" >&2; exit 1; }; \
	done
	install -d $(APP_DIR) $(DESTDIR)$(PREFIX)/bin $(SHARE)/applications \
		$(SHARE)/icons/hicolor/512x512/apps
	install -m 0755 $(BIN_DIR)/splitty-gpui $(BIN_DIR)/splitty $(APP_DIR)/
	rm -rf $(APP_DIR)/shell-integration
	cp -R shell-integration $(APP_DIR)/shell-integration
	find $(APP_DIR)/shell-integration -type d -exec chmod 0755 {} +
	find $(APP_DIR)/shell-integration -type f -exec chmod 0644 {} +
	ln -sf ../lib/splitty/splitty-gpui $(DESTDIR)$(PREFIX)/bin/splitty-gpui
	ln -sf ../lib/splitty/splitty $(DESTDIR)$(PREFIX)/bin/splitty
	sed 's|^Exec=.*|Exec=$(PREFIX)/bin/splitty-gpui|' packaging/linux/native/splitty.desktop \
		> $(SHARE)/applications/splitty.desktop
	chmod 0644 $(SHARE)/applications/splitty.desktop
	install -m 0644 crates/splitty-gpui/icons/icon.png $(SHARE)/icons/hicolor/512x512/apps/splitty.png
	@echo "Installed to $(DESTDIR)$(PREFIX)"

install-no-gui: $(BUILD)/splitty
	mkdir -p $(HOME)/.local/bin
	cp $(BUILD)/splitty $(HOME)/.local/bin/splitty
	@echo "Installed to $(HOME)/.local/bin/splitty"

# --- Uninstall ---

uninstall:
	rm -rf $(APP_DIR)
	rm -f $(DESTDIR)$(PREFIX)/bin/splitty-gpui $(DESTDIR)$(PREFIX)/bin/splitty \
		$(SHARE)/applications/splitty.desktop \
		$(SHARE)/icons/hicolor/512x512/apps/splitty.png
	@echo "Removed Splitty from $(DESTDIR)$(PREFIX)"

uninstall-no-gui:
	rm -f $(HOME)/.local/bin/splitty
	@echo "Removed $(HOME)/.local/bin/splitty"

uninstall-config:
	rm -rf $(HOME)/.config/splitty
	@echo "Removed $(HOME)/.config/splitty"

# --- Icons ---

icons:
	./scripts/generate-icons.sh

# --- Clean ---

clean:
	rm -rf $(BUILD)
	cargo clean
