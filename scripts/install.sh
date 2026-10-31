#!/usr/bin/env bash
set -euo pipefail

# Splitty installer for Linux
# Ships alongside the splitty binary and splitty-serve.service.
#
# Usage:
#   ./install.sh              Install binary only
#   ./install.sh --service    Install binary + systemd user service
#   ./install.sh --uninstall  Remove binary and service

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
INSTALL_DIR="${SPLITTY_INSTALL_DIR:-$HOME/.local/bin}"
SERVICE_DIR="$HOME/.config/systemd/user"
SERVICE_NAME="splitty-serve"

usage() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --service      Install binary and enable systemd user service"
    echo "  --uninstall    Remove binary and disable service"
    echo "  --help         Show this help"
    echo ""
    echo "Environment:"
    echo "  SPLITTY_INSTALL_DIR   Install directory (default: ~/.local/bin)"
}

install_binary() {
    if [ ! -f "$SCRIPT_DIR/splitty" ]; then
        echo "Error: splitty binary not found in $SCRIPT_DIR"
        exit 1
    fi

    mkdir -p "$INSTALL_DIR"
    cp "$SCRIPT_DIR/splitty" "$INSTALL_DIR/splitty"
    chmod +x "$INSTALL_DIR/splitty"
    echo "Installed splitty to $INSTALL_DIR/splitty"

    # Check if install dir is in PATH
    if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
        echo ""
        echo "WARNING: $INSTALL_DIR is not in your PATH."
        echo "Add this to your shell profile (~/.bashrc, ~/.zshrc, etc.):"
        echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    fi
}

install_service() {
    local service_src="$SCRIPT_DIR/splitty-serve.service"
    if [ ! -f "$service_src" ]; then
        echo "Error: splitty-serve.service not found in $SCRIPT_DIR"
        exit 1
    fi

    # Stop existing service if running
    systemctl --user stop "$SERVICE_NAME" 2>/dev/null || true

    mkdir -p "$SERVICE_DIR"

    # If binary is not in default location, patch ExecStart
    if [ "$INSTALL_DIR" != "$HOME/.local/bin" ]; then
        sed "s|%h/.local/bin/splitty|$INSTALL_DIR/splitty|" "$service_src" > "$SERVICE_DIR/$SERVICE_NAME.service"
    else
        cp "$service_src" "$SERVICE_DIR/$SERVICE_NAME.service"
    fi

    systemctl --user daemon-reload
    systemctl --user enable --now "$SERVICE_NAME"

    echo ""
    echo "Splitty serve is now running and will auto-start on login."
    echo ""
    echo "  status:  systemctl --user status $SERVICE_NAME"
    echo "  logs:    journalctl --user -u $SERVICE_NAME -f"
    echo "  stop:    systemctl --user stop $SERVICE_NAME"
}

uninstall() {
    # Stop and disable service
    systemctl --user disable --now "$SERVICE_NAME" 2>/dev/null || true
    rm -f "$SERVICE_DIR/$SERVICE_NAME.service"
    systemctl --user daemon-reload 2>/dev/null || true
    echo "Removed systemd service"

    # Remove binary
    rm -f "$INSTALL_DIR/splitty"
    echo "Removed $INSTALL_DIR/splitty"

    echo ""
    echo "Splitty has been uninstalled."
}

# --- Main ---

ACTION="binary"

while [ $# -gt 0 ]; do
    case "$1" in
        --service)  ACTION="service"; shift ;;
        --uninstall) ACTION="uninstall"; shift ;;
        --help|-h)  usage; exit 0 ;;
        *)
            echo "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

case "$ACTION" in
    binary)
        install_binary
        ;;
    service)
        install_binary
        install_service
        ;;
    uninstall)
        uninstall
        ;;
esac
