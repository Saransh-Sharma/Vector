# Architecture

Vector is a control plane, not a harness. The dependency direction is:

```text
schemas/core -> configuration/resolver/policy -> adapters/services
             -> vectord -> CLI/TUI/Tauri clients
```

The immutable `PortableRunSpec` is the boundary between intent and execution. Harness adapters compile it into `NativeRunPlan`; the harness remains responsible for its own model calls and agentic loop.

`vectord` owns runtime processes, approvals, jobs, canonical JSONL run artifacts, and SQLite projections. UI clients communicate through a versioned local protocol. The database is disposable and rebuildable; run artifacts are canonical.

Security settings use a restriction lattice. Ordinary configuration follows layer precedence, while capabilities select the most restrictive decision across the hard floor, user ceiling, repository, Packs, isolation substrate, and run grant.

