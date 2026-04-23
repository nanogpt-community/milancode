# MILANCODE.md

## Project Overview
MilanCode is a Rust agentic coding harness built around an interactive REPL, local tools, managed sessions, MCP servers, and a user-controlled permission model. It supports NanoGPT only. Web retrieval (search/scrape) always runs through Exa.

## Repository Shape
- Rust workspace at the repository root; `Cargo.toml` includes all crates under `crates/*`.
- Active refactor in progress: the main CLI crate lives at `crates/milancode/`. Verify `crates/milancode/Cargo.toml` defines the binary target.
- Notable crates:
  - `crates/milancode/` — intended main CLI binary.
  - `crates/api/` — client, types, errors; contains integration tests in `tests/`.
  - `crates/runtime/` — core runtime: bootstrap, config, conversation, MCP clients/stdio, sandbox, session, file ops, prompts.
  - `crates/commands/`, `crates/plugins/`, `crates/tools/` — command dispatch, plugin system, and tool implementations.
  - `crates/compat-harness/` — compatibility utilities.
- CI: `.github/workflows/release-milancode.yml`.
- Shared assistant settings live in `.milancode/`.

## Commands
- Build the workspace: `cargo build --workspace`
- Run all tests: `cargo test --workspace`
- Run a single crate's tests: `cargo test -p <crate-name>`
- Check formatting: `cargo fmt --all`
- Run lints: `cargo clippy --workspace` (respects workspace-level `forbid(unsafe_code)` and clippy
