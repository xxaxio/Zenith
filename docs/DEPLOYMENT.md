# Zenith Production Deployment Guide (Linux Systemd Daemon)

This guide documents the enterprise deployment, systemd integration, and Linux kernel optimization patterns for **Zenith** (high-performance zero-chunk virtual limbo & cyber-defense daemon).

---

## 1. Architectural Role: Proxy vs. Limbo Daemon

In high-throughput Minecraft networks:
- **Meridian (or Velocity / BungeeCord)** operates as the **Reverse Proxy and Network Gateway**: It listens on public port `:25565` and routes traffic across game servers (Lobbies, Survival, Minigames).
- **Zenith** operates as the **Zero-Chunk Crash Recovery & Virtual Limbo Daemon**: It holds connected players in a lightweight void state (< 1 KiB RAM per session) with animated BossBars during server restarts, network maintenance, or cyber attacks, then drains them smoothly into backend game instances via native `0x84 Transfer` packets.

Zenith is engineered to run as a **dedicated Linux system daemon** (`zenith.service`) alongside your proxy and game backends.

---

## 2. 1-Minute Quick Installation

On any modern Linux server (Ubuntu 20.04+, Debian 11+, RHEL/Rocky 9+):

```bash
curl -sSL https://raw.githubusercontent.com/xxaxio/Zenith/main/dist/install.sh | sudo bash
```

### What the installer automates:
1. Detects system CPU architecture (`x86_64` or `aarch64`).
2. Installs the release binary to `/usr/local/bin/zenith`.
3. Creates a dedicated, unprivileged system account (`zenith:zenith`) with no interactive shell.
4. Generates a production configuration at `/etc/zenith/zenith.toml` (never overwrites existing settings).
5. Installs and hardens the systemd service unit `/etc/systemd/system/zenith.service`.
6. Sets `LimitNOFILE=1048576` to ensure millions of simultaneous socket connections can be sustained.
7. Enables the daemon to launch on system boot.

---

## 3. Systemd Service Lifecycle Management

Control the Zenith daemon using standard Linux service utilities:

```bash
# Check running status and resource consumption
sudo systemctl status zenith

# View live streaming logs with nanosecond timestamps
sudo journalctl -u zenith -f

# Zero-downtime hot-reload configuration (sends SIGHUP)
sudo systemctl reload zenith

# Restart the service
sudo systemctl restart zenith

# Stop the daemon
sudo systemctl stop zenith
```

### Zero-Downtime Configuration Reloads (`SIGHUP`)
Zenith handles `SIGHUP` signals cleanly without dropping active player sessions or resetting queues:
```bash
sudo kill -HUP $(pidof zenith)
# Or via systemd:
sudo systemctl reload zenith
```
When reloaded, Zenith reparses `/etc/zenith/zenith.toml`, updates rate-limiting parameters, adjustments to wave sizes, and logging levels instantaneously.

---

## 4. Linux Kernel Tuning for High Concurrency (100k+ Sessions)

To sustain high-density loads (such as our empirically validated **85,205 concurrent sessions**), configure kernel socket and file-descriptor limits in `/etc/sysctl.d/99-zenith.conf`:

```ini
# /etc/sysctl.d/99-zenith.conf

# Maximum open file descriptors
fs.file-max = 2097152

# TCP socket backlog and accept queue depth
net.core.somaxconn = 65535
net.ipv4.tcp_max_syn_backlog = 65535

# Ephemeral client port range
net.ipv4.ip_local_port_range = 1024 65535

# Fast socket recycling and reuse
net.ipv4.tcp_tw_reuse = 1
net.ipv4.tcp_fin_timeout = 15

# Memory buffer allocation for TCP (min default max in pages)
net.ipv4.tcp_mem = 786432 1048576 1572864
net.ipv4.tcp_rmem = 4096 87380 16777216
net.ipv4.tcp_wmem = 4096 65536 16777216
```

Apply immediately without rebooting:
```bash
sudo sysctl --system
```

---

## 5. Deployment Topologies

### Topology A: Behind Meridian / Velocity Proxy (Recommended for Networks)

```
[Internet Players]
       │
       ▼ (Port :25565)
[Meridian Proxy]
   ├── Normal Flow ──────────► [Horizon / Paper Backend] (Port :25566)
   └── Maintenance / Full ───► [Zenith Limbo Daemon] (Port :25550)
                                      │
                                      └── 0x84 Drain ───► [Horizon Backend]
```

1. Meridian routes incoming players directly to backend servers.
2. If a backend restarts or triggers failover, Meridian routes players into **Zenith** on port `:25550`.
3. Zenith holds players in the virtual void, provides BossBar status, and drains them into `:25566` as soon as the health checks recover.

### Topology B: Standalone Fronting (Single Dedicated Server)

```
[Internet Players]
       │
       ▼ (Port :25565 Public)
[Zenith Daemon]
       │ (Auto-Drain / Proxy Mode)
       ▼ (Port :25566 Localhost)
[Paper / Fabric Server]
```

Zenith listens on public port `:25565`, sheltering the single server instance from DDoS attacks, slowloris scans, and connection storms while buffering players during restarts.

---

## 6. Upgrades & Uninstallation

### Upgrading to the Latest Release
Re-running the installation script cleanly updates the binary while preserving all configurations:
```bash
curl -sSL https://raw.githubusercontent.com/xxaxio/Zenith/main/dist/install.sh | sudo bash
```

### Complete Uninstallation
To remove Zenith while keeping configuration files:
```bash
sudo /usr/local/bin/zenith-uninstall || sudo ./dist/uninstall.sh
```

To purge all files including configurations in `/etc/zenith`:
```bash
sudo ./dist/uninstall.sh --purge
```
