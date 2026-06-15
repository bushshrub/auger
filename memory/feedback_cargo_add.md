---
name: feedback-cargo-add
description: Use cargo add for adding deps, not manual Cargo.toml edits
metadata:
  type: feedback
---

Use `cargo add` to add dependencies, not manual Cargo.toml edits.

**Why:** User preference — cargo add handles version resolution correctly and is the idiomatic tool.

**How to apply:** Any time adding a new dependency to Cargo.toml, run `cargo add <crate>` (with `--package` for workspace members or `-p` flag). For workspace-level deps use `cargo add --package <member> <crate>` and then move to workspace if needed, or check if cargo supports `--workspace` flag for that.
