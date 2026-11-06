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

---

## 4. Running Zenith

### Direct Execution
```sh
# Start with default bind address (127.0.0.1:25565)
./target/release/zenith

# Specify a custom bind address and logging level
ZENITH_BIND="0.0.0.0:25565" RUST_LOG="info" ./target/release/zenith
```

### Windows PowerShell
```powershell
$env:RUST_LOG="info"
$env:ZENITH_BIND="127.0.0.1:25565"
.\target\release\zenith.exe
```

### CLI Flags
- `--version` / `-V`: Prints version and exits.
- `--check`: Validates configuration and internal runtime without binding socket.

---

## 5. Quality & Test Suite

Zenith enforces a zero-warning policy across all crates:

```sh
# 1. Run all workspace tests (unit and integration)
cargo test --workspace

# 2. Strict lint check (must produce 0 warnings)
cargo clippy --workspace --all-targets -- -D warnings

# 3. Code formatting verification
cargo fmt --check
```
