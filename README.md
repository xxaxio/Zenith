# Zenith

**An ultra-high-density virtual dimension, queue, cybersecurity & fallback engine in Rust for the Horizon and Minecraft ecosystem.**

Zenith is a specialized, zero-physical-world virtual server designed to hold, queue, protect, and seamlessly route tens of thousands of players across Minecraft and [Horizon](https://github.com/horizon-rust/horizon) server networks in coordination with modern proxies like [Meridian](https://github.com/Akirabane/Meridian).

---

## Features & Status Matrix

| Component | Status | Description |
|---|---|---|
| **Zero-Chunk Dimension** | Done | Minimal void/sky dimension registry without terrain storage (`zenith-session`) |
| **Lightweight Async Actors** | Done | Bounded Tokio task per connection with sub-1KiB memory footprint (`zenith-session`) |
| **Priority Queue Scheduler** | Planned | FIFO ordering with VIP overrides and rate-controlled draining (`zenith-transit`) |
| **Dynamic HUD Display** | Planned | Real-time BossBar progression, Actionbar ETA text, and sound cues (`zenith-display`) |
| **Defensive Cybersecurity** | Done | Anti-Slowloris socket timers, strict frame size limits, rate limiting (`zenith-shield`) |
| **Totem Crash Recovery** | Planned | Lifeline evacuation and automatic backend re-routing upon recovery (`zenith-totem`) |
| **HMAC Forwarding Support** | Planned | Cryptographic verification of forwarded player identities (`zenith-wire`) |
| **Seamless Transfer** | Planned | Client-side transfer packet dispatch without disconnect or loading screens (`zenith-transit`) |
| **Prometheus Telemetry** | Planned | Real-time connection count, drain rate, and memory RSS metrics (`zenith-app`) |
| **Benchmark Harness** | In progress | Empirical load testing and memory profiling tool (`zenith-swarm`) |

*Status definitions: `Done` (merged & verified in CI), `In progress` (under active implementation), `Planned` (specified in design).*

---

## Architecture Topology

Zenith divides responsibilities into 8 specialized crates with `unsafe_code = "forbid"`:

```
crates/
├── zenith-wire       # Low-level protocol framing, VarInts, compression & encryption
├── zenith-session    # Zero-chunk virtual dimension, connection actor loops
├── zenith-transit    # Priority queue engine & destination dispatching
├── zenith-display    # BossBar, Actionbar, title and sound packet builders
├── zenith-shield     # Defensive security, anti-Slowloris, payload limits
├── zenith-totem      # Crash evacuation lifeline & backend health poller
├── zenith-app        # Production server executable daemon (main.rs)
└── zenith-swarm      # Synthetic bot benchmark generator
```

---

## Documentation

Exhaustive design specifications and operational manuals are available in `docs/`:

- [Architecture Specification](docs/architecture.md)
- Additional design manuals are continuously updated during development.

---

## License

MIT License. Copyright (c) 2026 xxaxio.
