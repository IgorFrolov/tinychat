# tinychat

Client with OpenAI-compatible Chat Completions API

## Supported OpenAI models

tinychat has explicit request profiles for these 10 popular and recommended
text-chat models:

1. `gpt-5.6-sol`
2. `gpt-5.6-terra`
3. `gpt-5.6-luna`
4. `gpt-5.5`
5. `gpt-5.4`
6. `gpt-5.4-mini`
7. `gpt-5.4-nano`
8. `gpt-5-mini`
9. `gpt-4.1-mini`
10. `gpt-4o-mini`

OpenAI does not publish a usage-based popularity ranking, so this is a
practical list combining the current recommended families with widely used
small chat models. These models and their dated snapshots use
`max_completion_tokens`. Reasoning models omit unsupported sampling parameters
and send the system prompt as a `developer` message. Other model IDs are still
accepted and use the legacy OpenAI-compatible `max_tokens` request shape.

The 10 models are available in the model selector by default. Override the
list with a comma-separated `OPENAI_MODELS` value or `--models`.

## Configuration

```
export HTTP_PROXY="http://127.0.0.1:8118"
export HTTPS_PROXY="http://127.0.0.1:8118"
export NO_PROXY="localhost,127.0.0.1,::1"
export OPENAI_API_KEY="sk-proj-xxxxxxxxx"
export OPENAI_MODEL="gpt-5.6-terra"
cargo run
```

### SOCKS5 proxy

Set `ALL_PROXY` to route both HTTP and HTTPS requests through SOCKS5:

```sh
export ALL_PROXY="socks5h://127.0.0.1:1080"
export NO_PROXY="localhost,127.0.0.1,::1"
export OPENAI_API_KEY="sk-proj-xxxxxxxxx"
export OPENAI_MODEL="gpt-5.6-terra"
cargo run
```

Both `socks5://` and `socks5h://` URLs are supported, including
`socks5h://user:password@127.0.0.1:1080`. Use `socks5h://` when DNS lookups
should also be performed through the proxy. `HTTP_PROXY` and `HTTPS_PROXY`
override `ALL_PROXY` for their respective protocols; lowercase variable names
are supported too.
