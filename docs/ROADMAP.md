# Delivery status and production gates

## Implemented vertical slice

- Public configuration and lock types.
- Deterministic resolver and policy lattice.
- LM Studio/Ollama discovery.
- OMP, Pi, and DeepSeek native plan compilers.
- Run artifacts, SQLite projection, daemon IPC, CLI/TUI, and desktop foundation.
- Built-in Pack manifests and onboarding-generated Safe/YOLO profiles.

## Required production gates

- Complete OMP ACP, Pi RPC/SDK, and DeepSeek JSON-RPC conformance suites against the pinned packages.
- Signed macOS computer-use helper, then Linux and Windows helpers.
- Kernel-backed workspace sandbox providers and container/VM lifecycle.
- Durable download, calibration, recipe, eval, benchmark, and optimization workers.
- Full pack signature verification and immutable remote source fetchers.
- Notarized/signed multi-platform installers, SBOMs, rollback manifests, and release channels.
- Scale validation at 100,000 runs and 1,000,000 events.

These gates are intentionally explicit. Unsupported enforcement is reported as unsupported; it is never simulated by prompt text.

