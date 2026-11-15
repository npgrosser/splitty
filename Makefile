UNAME   := $(shell uname -s)
BUILD   := build
COMMIT  := $(shell git rev-parse --short HEAD 2>/dev/null || echo unknown)

.PHONY: clean test icons native-package \
       install-no-gui uninstall-no-gui uninstall-config

# --- Build targets ---

$(BUILD):
	mkdir -p $@

$(BUILD)/splitty: | $(BUILD)
	SPLITTY_COMMIT=$(COMMIT) SPLITTY_BUILD_TIME=$$(date -u +%Y-%m-%dT%H:%M:%SZ) \
		cargo build --release -p splitty-server
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

# --- Install ---

install-no-gui: $(BUILD)/splitty
	mkdir -p $(HOME)/.local/bin
	cp $(BUILD)/splitty $(HOME)/.local/bin/splitty
	@echo "Installed to $(HOME)/.local/bin/splitty"

# --- Uninstall ---

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
