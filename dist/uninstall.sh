#!/usr/bin/env bash
# ==============================================================================
# Zenith Production Uninstaller (Linux Systemd Daemon)
# Repository: https://github.com/xxaxio/Zenith
#
# Usage:
#   sudo ./dist/uninstall.sh [--purge]
# ==============================================================================

set -euo pipefail

INSTALL_BIN="/usr/local/bin/zenith"
CONFIG_DIR="/etc/zenith"
SERVICE_FILE="/etc/systemd/system/zenith.service"
PURGE=0

RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warn() {
    echo -e "${CYAN}[WARN]${NC} $1"
}

log_err() {
    echo -e "${RED}[ERROR]${NC} $1" >&2
}

if [ "$(id -u)" -ne 0 ]; then
    log_err "This uninstallation script must be run as root (or via sudo)."
    exit 1
fi

while [[ $# -gt 0 ]]; do
    case "$1" in
        --purge)
            PURGE=1
            shift
            ;;
        -h|--help)
            echo "Zenith Uninstaller"
            echo "Options:"
            echo "  --purge    Completely remove all configuration and log files in /etc/zenith"
            echo "  -h, --help Show this help dialog"
            exit 0
            ;;
        *)
            log_err "Unknown option: $1"
            exit 1
            ;;
    esac
done

echo -e "${BOLD}${RED}"
echo "=================================================================="
echo "          ZENITH DAEMON - PRODUCTION UNINSTALLER                  "
echo "=================================================================="
echo -e "${NC}"

# 1. Stop and disable systemd service
if systemctl is-active --quiet zenith 2>/dev/null; then
    log_info "Stopping zenith.service..."
    systemctl stop zenith
fi

if systemctl is-enabled --quiet zenith 2>/dev/null; then
    log_info "Disabling zenith.service..."
    systemctl disable zenith
fi

# 2. Remove systemd service unit
if [ -f "${SERVICE_FILE}" ]; then
    log_info "Removing ${SERVICE_FILE}..."
    rm -f "${SERVICE_FILE}"
    systemctl daemon-reload
fi

# 3. Remove binary
if [ -f "${INSTALL_BIN}" ]; then
    log_info "Removing binary ${INSTALL_BIN}..."
    rm -f "${INSTALL_BIN}"
fi

# 4. Handle configuration directory
if [ -d "${CONFIG_DIR}" ]; then
    if [ "${PURGE}" -eq 1 ]; then
        log_warn "Purging configuration directory: ${CONFIG_DIR}..."
        rm -rf "${CONFIG_DIR}"
    else
        log_info "Preserving configuration files in ${CONFIG_DIR}."
        log_info "To remove configurations permanently, run with '--purge' or execute: rm -rf ${CONFIG_DIR}"
    fi
fi

# 5. Remove system user/group
if getent passwd zenith >/dev/null 2>&1; then
    log_info "Removing system user 'zenith'..."
    userdel zenith 2>/dev/null || true
fi

if getent group zenith >/dev/null 2>&1; then
    log_info "Removing system group 'zenith'..."
    groupdel zenith 2>/dev/null || true
fi

log_success "Zenith daemon has been completely uninstalled."
