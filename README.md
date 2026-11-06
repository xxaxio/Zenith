# Zenith

**A fast, lightweight limbo and queue server in Rust for Horizon and Meridian.**

Zenith is a standalone limbo and queue server built in Rust for [Horizon](https://github.com/horizon-rust/horizon) and [Meridian](https://github.com/Akirabane/Meridian) networks. It holds players in a lightweight virtual void, runs an adaptive priority queue, and transfers players seamlessly when backend servers restart or become available.

---

## Features & Status Matrix

| Component | Status | Description |
|---|---|---|
| **Zero-Chunk Dimension** | Done (Phase 2) | Minimal void/sky dimension registry without terrain storage (`zenith-session`) |
| **Lightweight Async Actors** | Done (Phase 2) | Bounded Tokio task per connection with sub-1KiB memory footprint (`zenith-session`) |
| **Defensive Cybersecurity** | Done (Phase 2) | Anti-Slowloris socket timers, strict frame size limits, rate limiting (`zenith-shield`) |
| **Priority Queue Scheduler** | Done (Phase 3) | Health-adaptive wave queue, VIP priority & ping sorting (`zenith-transit`) |
| **Dynamic HUD Display** | Done (Phase 3) | Protocol 777 BossBar, SmallCaps Actionbar, sound cues & instant transfer (`zenith-display`) |
| **Totem Crash Recovery** | Planned (Phase 4) | Lifeline evacuation and automatic backend re-routing upon recovery (`zenith-totem`) |
| **HMAC Forwarding Support** | Planned (Phase 4) | Cryptographic verification of forwarded player identities (`zenith-wire`, `zenith-shield`) |
| **Seamless Transfer** | Planned (Phase 4) | Client-side transfer packet dispatch without disconnect or loading screens (`zenith-transit`) |
| **Prometheus Telemetry** | Planned (Phase 5) | Real-time connection count, drain rate, and memory RSS metrics (`zenith-app`) |
| **Synthetic Testkit & Mocks** | Planned (Phase 5) | Scripted bot actors and mock Horizon backends for integration testing (`zenith-testkit`) |
| **Benchmark Harness** | Planned (Phase 6) | Multi-scenario load testing, bot simulations, and memory profiling tool (`zenith-bench`) |

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

*Test utilities (`zenith-testkit`) are isolated in `[dev-dependencies]`, and the load-testing harness (`zenith-bench`) is an internal tool (`publish = false`) ensuring zero test footprint in production binaries.*

---

## Documentation

Exhaustive design specifications and operational manuals are available in `docs/`:

- [Architecture Specification](docs/architecture.md) — System boundaries, components, and topology.
- [Protocol 777 Reference](docs/PROTOCOL.md) — Exact packet mappings and state machine.
- [Building & Testing](docs/BUILD.md) — Setup, compilation, and test execution guide.

---

## License

MIT License. Copyright (c) 2026 xxaxio.
