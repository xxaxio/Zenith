# Zenith Architecture Specification

Zenith is a high-throughput, zero-chunk virtual holding and transit gatekeeper designed for the Minecraft / Horizon server ecosystem.

---

## 1. Modular Crate Topology

Zenith separates protocol parsing, actor lifecycle, queue scheduling, and security hardening across 8 specialized crates:

| Crate | Responsibility |
|---|---|
| `zenith-wire` | Low-level binary protocol codec (VarInts, framing, encryption, compression). |
| `zenith-session` | Zero-chunk virtual dimension holding, Tokio actor task per connection. |
| `zenith-transit` | Priority FIFO queue scheduler and transfer trigger. |
| `zenith-display` | HUD interface builder (animated BossBar, Actionbar, audio cues). |
| `zenith-shield` | Defensive cybersecurity layer (anti-Slowloris, frame bounds, rate-limits). |
| `zenith-totem` | Crash recovery and backend evacuation lifeline. |
| `zenith-app` | Production server daemon binary (`src/main.rs`). |
| `zenith-swarm` | Synthetic bot load testing harness and memory profiler. |

---

## 2. Invariants & Guarantees

1. **Zero Physics Overhead**: No chunk meshes, block entities, or pathfinding ticks are allocated.
2. **Deterministic Memory**: Sessions are bounded lightweight actors (< 1 KiB per idle session).
3. **Rust Strictness**: `unsafe_code = "forbid"` is enforced across all crates. No undocumented panics or unwrap in runtime paths.
4. **Proxy Interoperability**: Seamless integration with modern Minecraft proxies (Meridian, Velocity) via modern forwarding and client transfer primitives.
