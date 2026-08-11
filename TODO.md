# LiteLLM Rust integration plan

Tinychat already supports the required integration model: it sends streaming
`POST /v1/chat/completions` requests to any `OPENAI_BASE_URL`. Therefore,
integrate LiteLLM Rust as an external gateway rather than adding an unstable
SDK or crate to the binary.

> **Status note:** LiteLLM's official Rust migration is still in progress. The
> `/chat/completions` route is planned after the `/messages` migration, with a
> stated target of September 1, 2026. Do not make the proof-of-concept
> `LiteLLM-Labs/litellm-rust` repository a production dependency.

## Plan

- [ ] Define the MVP: support only streaming Chat Completions; exclude the
  Responses API, tool calls, and provider-specific parameters. Preserve the
  current OpenAI-compatible path as the fallback.
- [ ] Obtain the official Rust beta, once its `/v1/chat/completions` endpoint
  is available, and pin an exact image tag or revision. Verify its license and
  minimum supported Rust version.
- [ ] Prepare a minimal `litellm_config.yaml` containing model routes, keys
  supplied through environment variables, and a master key. Never commit
  secrets.
- [ ] Document gateway startup and Tinychat configuration in `README.md`:

  ```sh
  OPENAI_BASE_URL=http://127.0.0.1:4000/v1
  OPENAI_API_KEY=$LITELLM_MASTER_KEY
  OPENAI_MODEL=<route-name>
  ```

  Keep the existing CLI flags and environment-variable interface unchanged.

- [ ] Add contract tests against a local mock gateway covering the request URL,
  `Authorization` header, payload, SSE deltas, usage, `[DONE]`, 4xx/5xx
  errors, timeouts, cancellation, and existing response-size limits.
- [ ] Run an integration smoke test with the Rust gateway and one provider,
  without exposing credentials: normal streaming, response cancellation,
  invalid key, and unavailable model route.
- [ ] Add an opt-in CI job (manual or scheduled) that exercises the mock or a
  local gateway. Restrict live-provider checks to protected environments.
- [ ] Roll out in stages: experimental documentation, beta on a pinned
  gateway version, then a default recommendation after several compatible
  releases without regressions. Rollback remains a one-variable change to
  `OPENAI_BASE_URL`.

## Done criteria

Tinychat works through the official LiteLLM Rust gateway without UI or API
changes and retains streaming, SOCKS5 proxy support, timeouts, cancellation,
and bounded SSE response handling.

## References

- [LiteLLM Rust migration roadmap](https://docs.litellm.ai/blog/litellm-rust-launch)
- [LiteLLM Rust migration tracking issue](https://github.com/BerriAI/litellm/issues/31263)
