#!/usr/bin/env bash
# ==============================================================================
# Zenith Production Installer (Linux Systemd Daemon)
# Repository: https://github.com/xxaxio/Zenith
#
# Usage:
#   curl -sSL https://raw.githubusercontent.com/xxaxio/Zenith/main/dist/install.sh | sudo bash
#   sudo ./dist/install.sh [--local <path_to_binary>]
# ==============================================================================

set -euo pipefail

GITHUB_REPO="xxaxio/Zenith"
INSTALL_BIN="/usr/local/bin/zenith"
CONFIG_DIR="/etc/zenith"
CONFIG_FILE="${CONFIG_DIR}/zenith.toml"
SYSTEMD_DIR="/etc/systemd/system"
SERVICE_FILE="${SYSTEMD_DIR}/zenith.service"
LOCAL_BIN=""
NO_START=0

# Colors for terminal output
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

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

# 1. Root privilege check
if [ "$(id -u)" -ne 0 ]; then
    log_err "This installation script must be run as root (or via sudo)."
    exit 1
fi

# 2. Parse command-line flags
while [[ $# -gt 0 ]]; do
    case "$1" in
        --local)
            LOCAL_BIN="$2"
            shift 2
            ;;
        --no-start)
            NO_START=1
            shift
            ;;
        -h|--help)
            echo "Zenith Production Installer"
            echo "Options:"
            echo "  --local <path>   Use local precompiled binary instead of downloading"
            echo "  --no-start       Enable service but do not start immediately"
            echo "  -h, --help       Show this help dialog"
            exit 0
            ;;
        *)
            log_err "Unknown option: $1"
            exit 1
            ;;
    esac
done

echo -e "${BOLD}${CYAN}"
echo "=================================================================="
echo "          ZENITH DAEMON - PRODUCTION SYSTEMD INSTALLER            "
echo "=================================================================="
echo -e "${NC}"

# 3. Detect architecture
ARCH="$(uname -m)"
case "${ARCH}" in
    x86_64|amd64)
        TARGET_ARCH="x86_64-unknown-linux-gnu"
        ;;
    aarch64|arm64)
        TARGET_ARCH="aarch64-unknown-linux-gnu"
        ;;
    *)
        log_err "Unsupported CPU architecture: ${ARCH}. Zenith requires x86_64 or aarch64."
        exit 1
        ;;
esac
log_info "Detected architecture: ${ARCH} (${TARGET_ARCH})"

# 4. Install binary
if [ -n "${LOCAL_BIN}" ]; then
    if [ ! -f "${LOCAL_BIN}" ]; then
        log_err "Specified local binary does not exist: ${LOCAL_BIN}"
        exit 1
    fi
    log_info "Installing from local binary: ${LOCAL_BIN}"
    install -m 755 "${LOCAL_BIN}" "${INSTALL_BIN}"
else
    log_info "Fetching latest release binary from GitHub (${GITHUB_REPO})..."
    TEMP_DIR="$(mktemp -d)"
    trap 'rm -rf "${TEMP_DIR}"' EXIT

    RELEASE_JSON="$(curl -sSL "https://api.github.com/repos/${GITHUB_REPO}/releases/latest" 2>/dev/null || true)"
    DOWNLOAD_URL=""
    CHECKSUMS_URL=""

    if [ -n "${RELEASE_JSON}" ]; then
        DOWNLOAD_URL="$(echo "${RELEASE_JSON}" | grep -o "https://[^\"]*linux-x86_64[^\"]*\.tar\.gz" | head -n 1 || true)"
        CHECKSUMS_URL="$(echo "${RELEASE_JSON}" | grep -o "https://[^\"]*SHA256SUMS" | head -n 1 || true)"
    fi

    if [ -n "${DOWNLOAD_URL}" ]; then
        log_info "Downloading ${DOWNLOAD_URL}..."
        curl -sSL "${DOWNLOAD_URL}" -o "${TEMP_DIR}/zenith.tar.gz"

        # Verify SHA-256 integrity if manifest is published (ZENITH-SUPPLY-008)
        ARCHIVE_NAME="$(basename "${DOWNLOAD_URL}")"
        if [ -n "${CHECKSUMS_URL}" ]; then
            log_info "Verifying SHA-256 checksum against official release manifest..."
            if curl -sSL "${CHECKSUMS_URL}" -o "${TEMP_DIR}/SHA256SUMS"; then
                EXPECTED_SHA="$(grep "${ARCHIVE_NAME}" "${TEMP_DIR}/SHA256SUMS" | awk '{print $1}' || true)"
                if [ -n "${EXPECTED_SHA}" ]; then
                    if command -v sha256sum >/dev/null 2>&1; then
                        ACTUAL_SHA="$(sha256sum "${TEMP_DIR}/zenith.tar.gz" | awk '{print $1}')"
                    elif command -v shasum >/dev/null 2>&1; then
                        ACTUAL_SHA="$(shasum -a 256 "${TEMP_DIR}/zenith.tar.gz" | awk '{print $1}')"
                    else
                        log_warn "Neither sha256sum nor shasum is available; unable to compute checksum."
                        ACTUAL_SHA=""
                    fi

                    if [ -n "${ACTUAL_SHA}" ]; then
                        if [ "${EXPECTED_SHA}" != "${ACTUAL_SHA}" ]; then
                            log_err "SECURITY ALERT: SHA-256 verification FAILED!"
                            log_err "Expected: ${EXPECTED_SHA}"
                            log_err "Actual:   ${ACTUAL_SHA}"
                            exit 1
                        fi
                        log_success "SHA-256 integrity verified: ${ACTUAL_SHA}"
                    fi
                else
                    log_warn "Release archive '${ARCHIVE_NAME}' not listed in SHA256SUMS; proceeding with caution."
                fi
            else
                log_warn "Could not retrieve SHA256SUMS; proceeding without checksum verification."
            fi
        fi

        tar -xzf "${TEMP_DIR}/zenith.tar.gz" -C "${TEMP_DIR}"
        FOUND_BIN="$(find "${TEMP_DIR}" -type f -name "zenith" | head -n 1)"
        if [ -n "${FOUND_BIN}" ]; then
            install -m 755 "${FOUND_BIN}" "${INSTALL_BIN}"
        else
            log_err "Could not locate 'zenith' binary inside downloaded package."
            exit 1
        fi
    else
        log_warn "No prebuilt release found on GitHub. Checking if running from git workspace..."
        if [ -f "./target/release/zenith" ]; then
            log_info "Found local workspace binary: ./target/release/zenith"
            install -m 755 "./target/release/zenith" "${INSTALL_BIN}"
        elif [ -f "./zenith" ]; then
            log_info "Found current directory binary: ./zenith"
            install -m 755 "./zenith" "${INSTALL_BIN}"
        else
            log_err "Unable to find or download a native Zenith binary."
            log_err "Compile it locally with: cargo build --release -p zenith-app --bin zenith"
            exit 1
        fi
    fi
fi
log_success "Binary installed at ${INSTALL_BIN}"

# 5. Create unprivileged system user & group
if ! getent group zenith >/dev/null 2>&1; then
    log_info "Creating system group 'zenith'..."
    groupadd --system zenith
fi

if ! getent passwd zenith >/dev/null 2>&1; then
    log_info "Creating system user 'zenith'..."
    useradd --system --gid zenith --no-create-home --home-dir /var/empty --shell /bin/false zenith
fi

# 6. Initialize configuration directory
if [ ! -d "${CONFIG_DIR}" ]; then
    log_info "Creating configuration directory: ${CONFIG_DIR}"
    mkdir -p "${CONFIG_DIR}"
fi

# Set restrictive permissions (only root and zenith can read)
chown -R root:zenith "${CONFIG_DIR}"
chmod 750 "${CONFIG_DIR}"

if [ ! -f "${CONFIG_FILE}" ]; then
    log_info "Generating default configuration at ${CONFIG_FILE}..."
    if [ -f "zenith.example.toml" ]; then
        cp "zenith.example.toml" "${CONFIG_FILE}"
    elif [ -f "dist/zenith.example.toml" ]; then
        cp "dist/zenith.example.toml" "${CONFIG_FILE}"
    else
        cat <<'EOF' > "${CONFIG_FILE}"
# Zenith Production Configuration
[server]
bind = "0.0.0.0:25565"
target = "127.0.0.1:25566"
mode = "queue"
max_players = 250000

[queue]
wave_size = 25
wave_interval_ms = 1000
sorting = "ping"

[shield]
enabled = true
max_connections_per_ip = 50
ip_rate_limit = 20.0
violation_threshold = 300000
jail_duration_secs = 900
packet_burst_quota = 100

[totem]
enabled = true
probe_interval_ms = 1000
consecutive_successes = 2

[metrics]
enabled = true
bind = "0.0.0.0:9100"
EOF
    fi
    chown root:zenith "${CONFIG_FILE}"
    chmod 640 "${CONFIG_FILE}"
    log_success "Generated initial configuration at ${CONFIG_FILE}"
else
    log_info "Existing configuration preserved at ${CONFIG_FILE}"
fi

GROUPS_FILE="${CONFIG_DIR}/groups.toml"
if [ ! -f "${GROUPS_FILE}" ]; then
    log_info "Installing default priority groups configuration at ${GROUPS_FILE}..."
    if [ -f "groups.toml" ]; then
        cp "groups.toml" "${GROUPS_FILE}"
    elif [ -f "dist/groups.toml" ]; then
        cp "dist/groups.toml" "${GROUPS_FILE}"
    else
        cat <<'EOF' > "${GROUPS_FILE}"
# Priority group configuration (LuckPerms style).
# Players are ordered by descending weight in the transit queue (zenith-transit).

[[groups]]
name = "admin"
weight = 1000
is_staff = true
members = [] # Example: ["AdminUser"]

[[groups]]
name = "vip"
weight = 500
is_staff = false
members = [] # Example: ["VipPlayer"]

[[groups]]
name = "default"
weight = 0
is_staff = false
members = []
EOF
    fi
    chown root:zenith "${GROUPS_FILE}"
    chmod 660 "${GROUPS_FILE}"
    log_success "Configured groups file at ${GROUPS_FILE}"
else
    log_info "Existing groups configuration preserved at ${GROUPS_FILE}"
fi

# 7. Install systemd service unit
log_info "Installing systemd service unit..."
if [ -f "dist/systemd/zenith.service" ]; then
    cp "dist/systemd/zenith.service" "${SERVICE_FILE}"
elif [ -f "zenith.service" ]; then
    cp "zenith.service" "${SERVICE_FILE}"
else
    cat <<'EOF' > "${SERVICE_FILE}"
[Unit]
Description=Zenith Zero-Chunk Virtual Limbo & Cyber-Defense Daemon
Documentation=https://github.com/xxaxio/Zenith
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=zenith
Group=zenith
ExecStart=/usr/local/bin/zenith --config /etc/zenith/zenith.toml
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=3s
TimeoutStopSec=15s
Environment=RUST_LOG=info
Environment=RUST_BACKTRACE=1

LimitNOFILE=1048576

UMask=0077
NoNewPrivileges=yes
CapabilityBoundingSet=CAP_NET_ADMIN
AmbientCapabilities=CAP_NET_ADMIN
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
ProtectProc=invisible
ProcSubset=pid
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
SystemCallArchitectures=native
SystemCallFilter=@system-service

ReadOnlyPaths=/etc/zenith
ReadWritePaths=/etc/zenith/groups.toml

[Install]
WantedBy=multi-user.target
EOF
fi

chmod 644 "${SERVICE_FILE}"
systemctl daemon-reload
systemctl enable zenith
log_success "Systemd service enabled: ${SERVICE_FILE}"

# 8. Start service if requested
if [ "${NO_START}" -eq 0 ]; then
    log_info "Starting Zenith service..."
    systemctl restart zenith
    log_success "Zenith daemon is now ACTIVE and running."
fi

echo -e "\n${BOLD}${GREEN}=================================================================="
echo "          ZENITH SUCCESSFULLY INSTALLED & CONFIGURED              "
echo "==================================================================${NC}"
echo -e "Binary         : ${INSTALL_BIN}"
echo -e "Configuration  : ${CONFIG_FILE}"
echo -e "Service Unit   : ${SERVICE_FILE}"
echo -e ""
echo -e "Useful Commands:"
echo -e "  Check status : ${CYAN}sudo systemctl status zenith${NC}"
echo -e "  View logs    : ${CYAN}sudo journalctl -u zenith -f${NC}"
echo -e "  Reload conf  : ${CYAN}sudo systemctl reload zenith${NC}"
echo -e "  Restart      : ${CYAN}sudo systemctl restart zenith${NC}"
echo -e "  Stop daemon  : ${CYAN}sudo systemctl stop zenith${NC}"
echo -e "=================================================================="
