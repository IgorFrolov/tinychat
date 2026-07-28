# tinychat

Client with OpenAI-compatible Chat Completions API

## Configuration

```
export HTTP_PROXY="http://127.0.0.1:8118"
export HTTPS_PROXY="http://127.0.0.1:8118"
export NO_PROXY="localhost,127.0.0.1,::1"
export OPENAI_API_KEY="sk-proj-xxxxxxxxx"
export OPENAI_MODEL="openai/gpt-5.6-terra"
cargo run
```

### SOCKS5 proxy

Set `ALL_PROXY` to route both HTTP and HTTPS requests through SOCKS5:

```sh
export ALL_PROXY="socks5h://127.0.0.1:1080"
export NO_PROXY="localhost,127.0.0.1,::1"
cargo run
```

Both `socks5://` and `socks5h://` URLs are supported, including
`socks5h://user:password@127.0.0.1:1080`. Use `socks5h://` when DNS lookups
should also be performed through the proxy. `HTTP_PROXY` and `HTTPS_PROXY`
override `ALL_PROXY` for their respective protocols; lowercase variable names
are supported too.
