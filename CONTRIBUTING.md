# Contributing

Vector is organized around one hard boundary: Vector controls and observes native harnesses; it does not implement an agent loop. Changes must preserve deterministic resolution, monotonic policy, secret redaction, and the ability to rebuild SQLite projections from append-only run artifacts.

Before opening a pull request, run `cargo test --workspace` and `npm run build`.

