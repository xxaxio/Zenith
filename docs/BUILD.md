# Building & Testing Zenith

This document describes how to compile, test, and verify Zenith on supported platforms.

---

## 1. Prerequisites

- **Rust:** `1.85.0` or later (2024 edition support).
- **Cargo:** included with Rust toolchain.
- **Git:** required for pulling pinned Horizon protocol dependencies.
- **Operating Systems:**
  - Linux (`x86_64-unknown-linux-gnu`)
  - Windows 10/11 (`x86_64-pc-windows-msvc`)

---

## 2. Upstream Horizon Dependencies

Zenith depends on [Horizon](https://github.com/horizon-rust/horizon) for exact Minecraft 26.3 / Protocol 777 data structures. The dependency is pinned to an immutable git commit in the root `Cargo.toml`:

```toml
horizon-protocol = { git = "https://github.com/horizon-rust/horizon", rev = "41f822b659789780f58d32c63ddfb6887ea1b8bf" }
horizon-world = { git = "https://github.com/horizon-rust/horizon", rev = "41f822b659789780f58d32c63ddfb6887ea1b8bf" }
```

Cargo automatically clones and caches this dependency on first build.

---

## 3. Compilation

### Debug Build (Fast Compilation)
```sh
cargo build -p zenith-app
```
Binary output: `target/debug/zenith` (or `target/debug/zenith.exe` on Windows).

### Release Build (Maximal Performance & Density)
```sh
cargo build --release -p zenith-app
```
Binary output: `target/release/zenith` (or `target/release/zenith.exe` on Windows).

### Benchmarking Tool Build (`zenith-bench`)
```sh
cargo build --release -p zenith-bench
```
Binary output: `target/release/zenith-bench` (or `target/release/zenith-bench.exe` on Windows).

---

## 4. Running Zenith

### Direct Execution
```sh
# Start with configuration file
./target/release/zenith --config zenith.toml

# Specify custom parameters via CLI
./target/release/zenith --bind 0.0.0.0:25565 --target 127.0.0.1:25566 --mode queue --wave-size 10
```

### CLI Arguments & Flags
- `--config` / `-c <path>`: Path to `zenith.toml` configuration file.
- `--bind` / `-b <addr>`: Socket address to bind (e.g. `0.0.0.0:25565`).
- `--target` / `-t <addr>`: Target backend server to transfer players to.
- `--mode` / `-m <limbo|queue|stress>`: Operational mode.
- `--wave-size` / `-w <count>`: Number of players drained per batch wave.
- `--wave-interval <ms>`: Milliseconds between consecutive transfer waves.
- `--no-shield`: Disables rate-limiting for high-throughput local bot evaluation.
- `--version` / `-V`: Prints version and exits.
- `--check`: Validates configuration without binding socket.

---

## 5. Running Synthetic Benchmarks (`zenith-bench`)

```sh
# 1. Limbo Holding Benchmark (500 bots, 15s hold, 200 concurrency)
./target/release/zenith-bench limbo-holding 127.0.0.1:25565 500 15 200

# 2. Priority Queue Drain (250 bots drained to backend :25566)
./target/release/zenith-bench queue-drain 127.0.0.1:25565 127.0.0.1:25566 250 50

# 3. Cyber-Defense Adversary Assault (with external proxy pool)
./target/release/zenith-bench adversary-storm 127.0.0.1:25565 100 /path/to/proxies.txt 50

# 4. Realistic Production Mix (70% Vanilla, 20% Horizon, 10% Hostiles)
./target/release/zenith-bench realistic-mix 127.0.0.1:25565 200 10 50
```

---

## 6. Quality & Test Suite

Zenith enforces a zero-warning policy across all crates:

```sh
# 1. Run all workspace tests (unit and integration)
cargo test --workspace

# 2. Strict lint check (must produce 0 warnings)
cargo clippy --workspace --all-targets -- -D warnings

# 3. Code formatting verification
cargo fmt --check
```
