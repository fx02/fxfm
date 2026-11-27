# Audit notes

- 2026-11-26 polish pass: baseline `fb54db4` (0.1.16). Checks: `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace` (207 + 455 passed, Trash tests
  ignored), `node --test ui/` (237 passed). No baseline failures.
