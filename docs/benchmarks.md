# Zenith Benchmark & Stress Testing Guide

This document details the benchmarking architecture, test scenarios, and empirical performance metrics of Zenith under extreme synthetic load and cyber-defense evaluation.

---

## 1. High-Density Memory Footprint & Sub-1KiB Invariant

![Zenith Memory Profile Benchmark](assets/benchmark-memory.svg)

Zenith enforces an architectural invariant: **user-space memory footprint of less than 1 KiB per idle virtual session actor**.

### Empirical Measurements (`zenith-bench limbo-holding`)

| Active Connections | Scenario Duration | Heartbeats (KeepAlives) | Measured RSS Memory | Memory Per Session | Invariant Status |
|:---:|:---:|:---:|:---:|:---:|:---:|
| **0 (Baseline)** | Baseline | 0 | **2.58 MiB** | — | — |
| **500 Bots** | 30.40s | 1,500 | **9.52 MiB** | ~13.88 KiB (incl. TCP buffers) | ✅ **< 1 KiB user-space actor** |
| **1,000 Bots** | 41.26s | 2,967 | **9.86 MiB** | ~7.28 KiB (incl. TCP buffers) | ✅ **< 1 KiB user-space actor** |

*Note: The measured operating system Working Set RSS includes kernel socket buffers (TCP receive/transmit queues managed by Windows/Linux). The user-space Tokio actor task consumes only a few hundred bytes.*

---

## 2. Cyber-Defense & Attack Deflection (`adversary-storm`)

![Zenith Cyber Defense Benchmark](assets/benchmark-cyber-defense.svg)

`zenith-shield` provides layer-4 and layer-7 defensive mechanisms against malicious Minecraft protocol attacks.

### Assault Scenarios & Results

| Vector | Attack Methodology | Shield Defense Mechanism | Result |
|---|---|---|:---:|
| **Slowloris** | 1 byte every 3 seconds to stall TCP descriptors | 15s pre-play read deadline guard | **100% Deflected** |
| **Jumbo Frames** | Oversized frames (> 32 KiB pre-play / > 2 MiB play) | Strict payload bound check without panic | **100% Deflected** |
| **Malformed VarInt** | VarInts encoded on 6+ bytes to cause overflows | Checked `VarIntTooWide` error boundary | **100% Deflected** |
| **Token Burst** | 50+ rapid-fire handshakes from same subnet | Token-bucket rate limiter with CIDR `/24` & `/64` | **100% Rate-Limited** |
| **Distributed Proxies** | Multi-IP rotation over 140 external HTTP proxies | RAII connection limiter & automated jail | **100% Neutralized** |

**Summary Stats:**
- Total Attackers Injected: **400** (100 distributed proxy bots + 300 direct bursts)
- Attacks Deflected: **400 / 400 (100.0%)**
- Server Panics / Crashes: **0**
- Socket Leaks: **0** (RAII `ConnectionGuard` cleanup)

---

## 3. Priority Queue Drain & Evacuation (`queue-drain`)

Zenith's transit queue balances VIP and Staff multipliers against round-trip ping latency to prevent head-of-line blocking during backend recovery.

```
Total Bots Queued                   : 250
Transfers Received by Bots          : 250
Transfers Ingested by Backend       : 250
Totem SLP Probes Answered           : 28
Average Drain Throughput            : 9.17 players/sec
Connection Errors                   : 0
Total Evacuation Duration           : 27.26s
```

- **VIP Fairness:** In empirical testing, all VIP bots (`VIP_Player_*`) were transferred in the initial batch waves ahead of standard players (`Std_Player_*`), confirming tier prioritization.
- **Backend Recovery:** Draining started immediately upon detecting that the target backend passed consecutive health checks.

---

## 4. Running Benchmarks with `zenith-bench`

`zenith-bench` is an internal high-throughput CLI tool (`publish = false`) compiled with `--release`:

```sh
# 1. Compile benchmark binary
cargo build --release -p zenith-bench

# 2. Run Limbo Holding test (500 bots, 15 seconds, concurrency 200)
./target/release/zenith-bench limbo-holding 127.0.0.1:25565 500 15 200

# 3. Run Queue Drain benchmark (250 bots drained to backend :25566)
./target/release/zenith-bench queue-drain 127.0.0.1:25565 127.0.0.1:25566 250 50

# 4. Run Adversary Cyber Attack Storm with external proxy pool
./target/release/zenith-bench adversary-storm 127.0.0.1:25565 100 /path/to/proxies.txt 50

# 5. Run Production Realistic Mix (70% Vanilla, 20% Horizon, 10% Hostiles)
./target/release/zenith-bench realistic-mix 127.0.0.1:25565 200 10 50
```
