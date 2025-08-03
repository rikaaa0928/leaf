
# Agent Instructions

This document provides instructions for agents working on this repository.

## Build, Lint, and Test

- **Build (Release):** `cargo build --release`
- **Build (Debug):** `cargo build`
- **Run a single test:** `cargo test -p leaf --test <test_name> -- --nocapture`
- **Run all tests:** `make test` or `cargo test -p leaf -- --nocapture`
- **Lint:** `cargo clippy --all-targets --all-features -- -D warnings`
- **Format:** `cargo fmt --all`

## Code Style Guidelines

- **Imports:** Group imports by standard library, external crates, and internal modules.
- **Formatting:** Adhere to `rustfmt` standards. Run `cargo fmt --all` before committing.
- **Types:** Use explicit types. Leverage Rust's type system for safety.
- **Naming:** Follow Rust's naming conventions (e.g., `snake_case` for variables and functions, `PascalCase` for types).
- **Error Handling:** Use `Result` for recoverable errors and `panic!` for unrecoverable ones. Utilize the `?` operator for concise error propagation.
- **Comments:** Add comments to explain complex or non-obvious logic.
- **Dependencies:** Add new dependencies to the appropriate `Cargo.toml` file.
