# Meridian & Zenith Integration Guide

This guide details the integration architecture, packet flows, and configuration patterns between **[Meridian](https://github.com/Akirabane/Meridian)** (high-throughput Rust Minecraft reverse proxy) and **Zenith** (virtual limbo holding, priority queue scheduler, and cyber-defense daemon).

---

## 1. Architectural Topology & Roles

![Meridian & Zenith Integration Topology](assets/meridian-topology.svg)

In a high-density network deployment:
- **Meridian** acts as the public-facing gateway and connection router (port `:25577` or `:25565`).
- **Zenith** operates as the **zero-chunk crash recovery fallback & priority transit gatekeeper**:
  - When backend game servers (Horizon, Paper, Folia, Fabric) are restarting, updating, crashing, or under high load, Meridian automatically delegates incoming player connections to Zenith instead of disconnecting them.
  - Zenith absorbs connections into an ultra-low-memory virtual void (< 1 KiB RAM per session), renders dynamic HUD displays (BossBar, Actionbar ETA), defends against cyber attacks (Slowloris, bot floods), and organizes players into a priority-sorted wave queue.
  - When the destination backend recovers, Zenith seamlessly transfers players via the native Minecraft `0x84 Transfer` packet.

---

## 2. Cryptographic Player Forwarding (HMAC-SHA256)

To maintain end-to-end player identity authenticity across Meridian, Zenith, and downstream backends without relying on unauthenticated IP spoofing, Zenith supports **HMAC-SHA256 cryptographic token forwarding** (`zenith-wire::hmac`).

### Token Payload Specification
```
[UUID: 16 bytes] [Timestamp: 8 bytes big-endian] [Signature: 32 bytes HMAC-SHA256]
```

1. **Generation (Meridian or Zenith)**:
   - Payload is formed by concatenating the player's 128-bit UUID and current UNIX epoch timestamp in seconds.
   - HMAC-SHA256 is calculated over `UUID || Timestamp` using a shared cluster secret (`hmac_secret`).
2. **Verification & Drift Window**:
   - The recipient verifies the HMAC signature using the shared secret.
   - A configurable drift window (default: $\pm 30\text{ seconds}$) prevents replay attacks while tolerating clock jitter across nodes.
3. **Rust Implementation**:
   ```rust
   use zenith_wire::HmacForwarding;

   let hmac = HmacForwarding::new("shared_cluster_secret_key");
   let token = hmac.sign_token(player_uuid);
   assert!(hmac.verify_token(player_uuid, &token, 30));
   ```

---

## 3. Operational Flows

### Flow A: Backend Maintenance / Crash Evacuation
1. Backend server `:25566` becomes unresponsive or enters scheduled maintenance.
2. Meridian detects the offline status and routes newly connecting players to Zenith on `:25565`.
3. Zenith accepts connections, applies `zenith-shield` verification, and spawns a lightweight virtual actor in `zenith:limbo`.
4. Players receive real-time queue position in their BossBar, and estimated time remaining in their Actionbar.

### Flow B: Health Recovery & Adaptive Wave Evacuation
1. `zenith-totem` continuously polls backend `:25566` via Server List Ping (SLP) or Horizon Turbo IPC (`ctl.sock`).
2. Once the backend achieves 2 consecutive successful health checks, `zenith-transit` initiates queue evacuation.
3. Players are partitioned into waves according to their priority tier (Staff $\to$ VIP $\to$ Standard) and sorted by ping latency.
4. Zenith issues `ClientboundTransferPacket` (`0x84 / 132`):
   - Host: Target backend IP or Meridian routing port.
   - Port: Target backend port.
5. The Minecraft client transitions to the destination server client-side without returning to the main menu or experiencing a loading screen freeze.

---

## 4. Configuration Examples

### Zenith Configuration (`zenith.toml`)
```toml
[server]
bind = "0.0.0.0:25550"
# Target backend polled by Totem for health recovery (internal/local address)
target = "127.0.0.1:25561"
# Public proxy/gateway address sent to clients in 0x84 Transfer packet
transfer_target = "play.example.com:25577"
mode = "queue"
online_mode = false

[queue]
wave_size = 15
wave_interval_ms = 1000
sorting = "ping"
groups_file = "groups.toml"

[totem]
enabled = true
probe_interval_ms = 1000
consecutive_successes = 2

[forwarding]
hmac_secret = "your_production_secret_key_here"

[shield]
enabled = true
max_connections_per_ip = 5
ip_rate_limit = 5.0
jail_duration_secs = 900
```

### Meridian Fallback Route Example (`meridian.toml`)
```toml
listen = "0.0.0.0:25577"
accept-transfers = true # Enable to accept incoming 0x84 client transfers from Zenith
auth-mode = "online"
forwarding-secret = "your_production_secret_key_here"

default-backend = "lobby"
try = ["limbo"]

[[backends]]
name = "lobby"
address = "127.0.0.1:25561"
seamless-arrival = true

[[backends]]
name = "limbo"
address = "127.0.0.1:25550"
seamless-arrival = true
```

### Downstream Game Backend Requirements (`server.properties` & Paper)

When running behind Meridian and Zenith, downstream backends (Paper, Leaf, Purpur, Horizon) **must** be configured with:

1. **`server.properties`**:
   ```properties
   # Mandatory: Allow proxy player information forwarding
   online-mode=false

   # Mandatory in Minecraft 1.20.5+ / 26.3: Accept 0x84 Transfer packets
   accepts-transfers=true
   ```

2. **`config/paper-global.yml`** (for Paper/Leaf/Purpur):
   ```yaml
   proxies:
     velocity:
       enabled: true
       online-mode: false # Set true if proxy validates Mojang sessions
       secret: your_production_secret_key_here
   ```

> [!WARNING]
> If `online-mode=true` remains in `server.properties`, the backend will reject proxy player forwarding with:
> `the backend asks players to authenticate: it must run with online mode off and the player information forwarding on`.

---

## 5. Troubleshooting & Real-World Edge Cases

### Issue 1: "This server does not accept transfers" Disconnect
* **Cause**: In Minecraft 1.20.5+ and 26.3, servers reject incoming `Transfer` intent packets by default if `accepts-transfers=false`.
* **Fix**: Ensure `accepts-transfers=true` is set in the downstream server's `server.properties`.

### Issue 2: Infinite Transfer Loop (Proxy $\leftrightarrow$ Limbo)
* **Symptoms**: Player joins proxy, gets sent to Limbo, Limbo immediately transfers to proxy, proxy sends back to Limbo every second.
* **Cause**: 
  1. The destination backend is running, so Zenith's Totem health check marks it as `Healthy` and triggers the transfer drain.
  2. However, the backend rejects the player login (e.g. `online-mode=true` conflicting with Velocity forwarding, or whitelist/ban).
  3. The proxy triggers its fallback route and sends the player right back into Zenith Limbo.
* **Fix**: Ensure the backend's `online-mode=false` and modern forwarding secrets match the proxy, so the player can actually join the game once transferred.

### Issue 3: `Internal Exception: java.net.SocketException: Connection reset` on Connect
* **Cause**: Protocol version mismatch. Meridian 0.1.0 is compiled for Minecraft 26.3 (Protocol `777`). Connecting with a 1.21.11 client (Protocol `774`) causes an immediate socket drop.
* **Fix**: Ensure the client version matches the proxy protocol version (e.g. use Horizon or a 26.3 snapshot client), or employ a multi-version proxy layer.

### Issue 4: Panel / Daemon File Overwrite on Startup
* **Cause**: Central hosting panels (e.g. HeavyPanel) that synchronize local templates to remote node daemons may overwrite remote `server.properties` upon server restart if the template is not updated.
* **Fix**: Ensure configuration changes are applied to both the central panel server template and the active daemon files.

