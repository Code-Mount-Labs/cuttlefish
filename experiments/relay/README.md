# Relay transport experiment

This evaluates **frp 0.72.0** as Cuttlefish's self-hosted transport. It uses only
loopback sockets, dummy application responses, a temporary TLS certificate and a
temporary shared token. It is not a deployable multi-tenant relay. See
[the integration decision](../../docs/remote-access.md) for remaining work.

Prerequisites: Python 3.9+, OpenSSL with `req -addext`, and the official frp 0.72.0
binaries for the host OS/architecture. Download the matching archive from the
[official release](https://github.com/fatedier/frp/releases/tag/v0.72.0) and verify
it against `SHA256SUMS` before extracting. The script rejects other versions.

```sh
python3 experiments/relay/transport_smoke.py --binary-directory /absolute/path/to/extracted/frp
```

It runs real frps/frpc processes behind a local TLS terminator, approximating
Railway's HTTPS-to-HTTP edge arrangement. It checks two paths, query preservation,
SSE streaming, WebSocket upgrade/data, connector and server restarts, unknown
routes, invalid connector credentials, and TLS certificate verification. It
prints a JSON result and cleans up all owned resources, including on failure.
Expected runtime is under a minute. This checks transport compatibility; it does
not substitute for the Rust coverage gate, tenancy tests or live Railway tests.

Last observed run: 2026-11-07, macOS ARM64, all nine checks passed.
Windows remains deferred. Linux artifacts are recorded but this experiment has
not yet been run on Linux.
