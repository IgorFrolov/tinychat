# Tinychat: guide for agents

## Project overview

`tinychat` is a minimal full-screen terminal client for OpenAI-compatible
Chat Completions APIs. It is a single Rust 2021 binary; the supported minimum
Rust version is 1.80.

- `src/main.rs` owns terminal setup/cleanup and the async event loop.
- `src/app.rs` owns application state, keyboard handling, and request
  lifecycle.
- `src/api.rs` owns HTTP, streaming SSE decoding, request profiles, and
  response limits.
- `src/config.rs` and `src/proxy.rs` own CLI/environment configuration.
- `src/ui.rs`, `src/layout.rs`, and `src/markdown/` own rendering and layout.
- `src/model.rs` owns conversation/request data types; `src/pricing.rs`,
  `src/qr.rs`, and `src/mascot.rs` provide focused features.

## Working conventions

- Keep changes small and localized; preserve the current module boundaries.
- Prefer the standard library and existing dependencies. Ask before adding a
  production dependency.
- Keep API keys and other secrets out of code, logs, tests, fixtures, and
  documentation. `AppConfig` and proxy configuration deliberately redact
  secrets in `Debug` output; preserve that behavior.
- Maintain compatibility with OpenAI-compatible Chat Completions endpoints,
  streaming responses, SOCKS5 proxy support, and the configured timeout.
- The UI runs in terminal raw mode. Any change to setup, shutdown, or panic
  handling must leave the terminal restored (cursor visible and raw mode off).
- Preserve streaming limits and cancellation behavior in `src/api.rs`; do not
  replace bounded parsing with unbounded buffering.
- Treat `vendor/crossterm` as a deliberate local patch for Kitty keyboard
  protocol behavior. Do not modify or update it unless the task explicitly
  concerns that patch.

## Tests and validation

After changing Rust code, run:

```sh
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Run `cargo fmt --all` to apply formatting when needed. Add or update focused
unit tests in the module being changed, especially for parsing, key handling,
layout, streaming SSE behavior, configuration, and proxy logic.

## Manual smoke testing

Launching the TUI requires credentials and may call a live API. Do not make
live requests or expose environment values unless the task explicitly needs
it. When manual testing is authorized, use the configuration described in
`README.md` (for example `OPENAI_BASE_URL`, `OPENAI_API_KEY`, and
`OPENAI_MODEL`).
