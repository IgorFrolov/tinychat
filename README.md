# tinychat

Simple client with OpenAI-compatible API

## Supported OpenAI models

tinychat has explicit request profiles for these recommended text-chat models:

- `gpt-5.6-sol`
- `gpt-5.6-terra`
- `gpt-5.6-luna`

## Local commands

Use `/qr <text or URL>` to generate a scannable QR code directly in the
terminal. The command runs locally and is not sent to the configured model.

```text
/qr https://github.com/IgorFrolov/tinychat
```

## Configuration

### SOCKS5 proxy

Set `ALL_PROXY` to route both HTTP and HTTPS requests through SOCKS5:

```sh
export ALL_PROXY="socks5h://127.0.0.1:1080"
export NO_PROXY="localhost,127.0.0.1,::1"
export OPENAI_BASE_URL="https://api.openai.com/v1"
export OPENAI_API_KEY="sk-proj-xxxxxxxxx"
export OPENAI_MODEL="gpt-5.6-luna"
cargo run
```

