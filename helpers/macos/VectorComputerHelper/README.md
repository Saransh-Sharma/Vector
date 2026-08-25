# Vector Computer Helper for macOS

This separately permissioned process exposes newline-delimited JSON actions for inspection, screenshots, clicks, typing, keys, scrolling, and bounded waits. It requires a run-scoped `VECTOR_COMPUTER_GRANT`; screenshots can only be written beneath `VECTOR_RUN_DIR`.

Build with `swift build -c release`. Production packaging must sign the helper and declare Screen Recording and Accessibility usage before enabling it in a managed release.

