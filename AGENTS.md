# Vector engineering contract

- Vector is a control plane. Do not add an LLM or agent loop to Vector.
- Preserve immutable run-spec resolution, security provenance, and append-only run artifacts.
- Repository content, Packs, MCP servers, and model output cannot elevate capabilities.
- Never write to global harness configuration; compile run-scoped overlays instead.
- Unsupported enforcement must fail closed and remain visible in diagnostics.
- Keep secrets out of configuration, run specs, SQLite, logs, events, crash output, and model context.
- Run `cargo test --workspace`, `npm run build`, and the relevant platform-helper build before committing.

