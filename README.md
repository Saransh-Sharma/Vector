# Vector

Vector is a local-first control plane for agent harnesses. It resolves a repository and user configuration into an immutable run specification, applies monotonic capability policy, compiles the result into native OMP, Pi, or DeepSeek Harness plans, and records what happened without replacing the harness agent loop.

## What works in this foundation

- Layered `vector.dev/v1alpha1` workspace configuration with field provenance.
- Monotonic `deny > prompt > allow` capability policy and explicit YOLO grants.
- Canonical, BLAKE3-addressed `PortableRunSpec` values.
- LM Studio and Ollama discovery, including exact model IDs. For loopback LM Studio endpoints, Vector uses the official `lms` CLI to start a stopped local API server automatically.
- Native plan compilation for OMP, Pi RPC, and DeepSeek Harness preview.
- Append-only run ledgers with a rebuildable SQLite read model.
- Authenticated, newline-delimited local IPC for `vectord` on Unix systems.
- `vctr` CLI, interactive TUI, first-run wizard, and Tauri desktop workbench.

Vector does not edit global harness configuration and does not collect telemetry.

## Build

```sh
cargo test --workspace
npm install
npm run build
```

Run the CLI directly during development:

```sh
cargo run -p vector-agent -- doctor
cargo run -p vector-agent -- init
cargo run -p vector-agent -- resolve --explain
cargo run -p vector-agent -- harness plan --profile pi-safe
```

Start the daemon:

```sh
cargo run -p vectord
```

The canonical installed binary is `vector-agent`; release packaging also installs the `vctr` alias. Vector intentionally does not claim the bare `vector` command.

## Project status

This repository implements the first executable vertical slice of the full Vector vision. See [ROADMAP.md](docs/ROADMAP.md) for implemented boundaries and the remaining production gates, including signed computer-use helpers, full harness conformance, and multi-platform packaging.

## License

MIT
