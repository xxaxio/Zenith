# Zenith

**An ultra-high-density virtual dimension, queue, cybersecurity & fallback engine in Rust for the Horizon and Minecraft ecosystem.**

Zenith is a specialized, zero-physical-world virtual server designed to hold, queue, protect, and seamlessly route tens of thousands of players across Minecraft and [Horizon](https://github.com/horizon-rust/horizon) server networks in coordination with modern proxies like [Meridian](https://github.com/Akirabane/Meridian).

---

## Features & Status Matrix

| Component | Status | Description |
|---|---|---|
| **Zero-Chunk Dimension** | Done (Phase 2) | Minimal void/sky dimension registry without terrain storage (`zenith-session`) |
| **Lightweight Async Actors** | Done (Phase 2) | Bounded Tokio task per connection with sub-1KiB memory footprint (`zenith-session`) |
| **Defensive Cybersecurity** | Done (Phase 2) | Anti-Slowloris socket timers, strict frame size limits, rate limiting (`zenith-shield`) |
| **Priority Queue Scheduler** | In progress (Phase 3) | FIFO ordering with VIP overrides and rate-controlled draining (`zenith-transit`) |
| **Dynamic HUD Display** | In progress (Phase 3) | Real-time BossBar progression, Actionbar ETA text, and sound cues (`zenith-display`) |
| **Totem Crash Recovery** | Planned (Phase 4) | Lifeline evacuation and automatic backend re-routing upon recovery (`zenith-totem`) |
| **HMAC Forwarding Support** | Planned (Phase 4) | Cryptographic verification of forwarded player identities (`zenith-wire`, `zenith-shield`) |
| **Seamless Transfer** | Planned (Phase 4) | Client-side transfer packet dispatch without disconnect or loading screens (`zenith-transit`) |
| **Prometheus Telemetry** | Planned (Phase 5) | Real-time connection count, drain rate, and memory RSS metrics (`zenith-app`) |
| **Benchmark Harness** | Planned (Phase 6) | Empirical load testing and memory profiling tool (`zenith-bench`) |

*Status definitions: `Done` (merged & verified in CI), `In progress` (under active implementation), `Planned` (scheduled in project phases).*

---

## Architecture Topology

Zenith divides responsibilities into focused member crates with `unsafe_code = "forbid"`:

```
crates/
├── zenith-wire       # Low-level protocol framing, VarInts, compression & encryption
├── zenith-session    # Zero-chunk virtual dimension, connection actor loops
├── zenith-transit    # Priority queue engine & destination dispatching
├── zenith-display    # BossBar, Actionbar, title and sound packet builders
├── zenith-shield     # Defensive security, anti-Slowloris, payload limits
├── zenith-totem      # Crash evacuation lifeline & backend health poller
└── zenith-app        # Production server executable daemon (main.rs)
```

---

## Documentation

Exhaustive design specifications and operational manuals are available in `docs/`:

- [Architecture Specification](docs/architecture.md) — System boundaries, components, and topology.
- [Protocol 777 Reference](docs/PROTOCOL.md) — Exact packet mappings and state machine.
- [Building & Testing](docs/BUILD.md) — Setup, compilation, and test execution guide.

---

## License

MIT License. Copyright (c) 2026 xxaxio.
