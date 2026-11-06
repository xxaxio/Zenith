# Zenith

**An ultra-high-density virtual dimension, queue & fallback engine in Rust for Horizon and Meridian.**

Zenith is a lightweight, zero-physical-world virtual server designed to hold, queue, and seamlessly route tens of thousands of players across [Horizon](https://github.com/horizon-rust/horizon) server networks in coordination with proxies like [Meridian](https://github.com/Akirabane/Meridian).

---

## The Vision

In large-scale Minecraft networks, standard game servers cannot afford to hold 10,000+ players in queues or during unexpected server restarts without catastrophic memory and CPU degradation. 

Zenith solves this by acting as a high-throughput **virtual holding layer**:
- **Zero World Overhead**: No physics ticking, no terrain meshing, no block collision calculations.
- **Ultra-High Density**: Capable of sustaining 50,000+ concurrent connections under minimal memory usage.
- **Interactive Queue**: Real-time feedback for waiting players with dynamic BossBars, action bar status (`Position #12 / 300 - Est. wait 15s`), sound cues, and interactive commands.
- **Seamless Hand-off**: Cooperates with Meridian's transfer flow to transition players to their destination Horizon backend cleanly.
- **Crash & Evacuation Shield**: Instantly catches and stabilizes players when a backend server restarts or crashes, holding their sessions safely until recovery.

---

## Status

Zenith is currently in the **active design and architectural specification phase**.

- **Protocol**: Protocol 777 (Minecraft 26.3) & Horizon native transfer primitives
- **Runtime**: Pure Rust (Tokio async I/O, zero-copy packet handling)
- **License**: MIT

---

## Authors

Created by **xxaxio**.
