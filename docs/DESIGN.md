# Design

## Goals

1. A Windows-only MITM proxy that is useful without a browser plugin or a separate packet capture stack.
2. A mitmweb-like analysis UI embedded into one executable.
3. First-class machine access: AI agents must be able to discover, filter, read, decode, export and control captures through MCP, not just scrape a UI.
4. Format switching must be explicit and repeatable: a human or agent can ask for `grpc`, `protobuf`, `json`, `http`, `raw`, etc. against the same captured bytes.

## Process model

`mitmrs.exe` is one Tokio process:

- Axum serves the embedded UI, REST API, SSE and MCP on `web_port` (default 8787).
- A proxy engine listens on `proxy_port` (default 8080).
- A SQLite database stores flows.
- An optional WinDivert worker thread rewrites packets and hands connections to the proxy listener.
- The CA, config and database live in the data dir (`%LOCALAPPDATA%\MitmRs` by default).

The process keeps running if the proxy fails to bind, so the UI can show the error and let the user change the port. Fatal errors (web port in use) exit.

## Explicit proxy and TLS MITM

The proxy is an HTTP/1.1 proxy for ordinary requests and CONNECT:

1. A plain request has an absolute URI; it is forwarded after hop-by-hop header filtering.
2. CONNECT receives `200 Connection Established`.
3. The proxy reads the first byte from the upgraded stream.
4. If it is a TLS handshake (`0x16`), the proxy terminates TLS with a per-host certificate signed by the local CA and serves HTTP/1.1 or HTTP/2 based on ALPN.
5. If it is not TLS, the proxy tunnels raw bytes (no application capture), preserving the first byte.

HTTPS interception requires the client to trust the local CA. Certificates are cached per host and generated on demand. The CA and key are persisted; the root CA is generated with a **10-year (3650-day)** lifetime, backdated one day for clock skew. Deleting the `ca` directory creates a new identity (clients must re-trust it).

Ordinary web traffic is forwarded to upstream as HTTP/1.1 even when the downstream client uses HTTP/2; hyper translates the response back to h2. This avoids gateways (for example APISIX) rejecting h2-translated h1 requests and supports origins that only speak h1. Native gRPC is the exception: it requires end-to-end HTTP/2, with a separate h2 client (and an h2c client for cleartext prior-knowledge gRPC). gRPC requires `te: trailers`, so that one hop-by-hop header is intentionally preserved. Separate connection pools per protocol prevent cross-version reuse.

## Transparent WinDivert redirect

WinDivert sits at `WINDIVERT_LAYER_NETWORK`. The filter is:

```
outbound and ip and tcp and (
  (!loopback and (DstPort==80 or DstPort==443)
   and !(SrcPort >= 30000 and SrcPort <= 39999))
  or (loopback and SrcPort == <proxy_port>)
)
```

For each new outbound flow the redirector allocates a fake client source port (`relay_port`, 20000–29999) and stores:

```
(client_ip, client_port, dst_ip, dst_port) -> {relay_port, ifIdx, ...}
relay_port -> (client_ip, client_port, dst_ip, dst_port)
```

The SYN is rewritten to `127.0.0.1:<relay_port> -> 127.0.0.1:<proxy_port>` and re-injected on the loopback interface. Subsequent client packets are matched by the original tuple and rewritten the same way. The proxy accepts a socket whose peer port is `relay_port`, looks up the original destination, and uses SNI/Host for TLS/HTTP. The proxy's reply leaves from `127.0.0.1:<proxy_port>` to `127.0.0.1:<relay_port>`; the redirector rewrites it to `dst_ip:dst_port -> client_ip:client_port` and injects it as inbound, so the client's TCP stack sees the original server.

The proxy's own upstream sockets bind to 30000–39999, which the filter excludes. This is simpler and faster than querying the TCP owner PID for every packet. The trade-off is documented: an unrelated application using a source port in that range bypasses transparent interception.

Only IPv4/TCP and ports 80/443 are handled. The filter deliberately excludes loopback traffic other than proxy replies.

## Capture pipeline

Request path:

1. Resolve the absolute target (scheme/host/port/path) from the URI, Host header, and connection metadata.
2. Collect the request body (bounded at 128 MB) and retain the first `max_body_bytes` for storage.
3. Forward to upstream.
4. Build a `FlowRecord` with request data and response status/headers.
5. Insert it immediately, emit an SSE `flow` event, then stream the response body to the client while teeing up to `max_body_bytes` into memory.
6. On completion, update the row with the captured body, trailers, total size, truncation flag, duration and any stream error; emit another `flow` event.

This keeps streaming/SSE/gRPC working and makes long-lived responses visible before they finish. The response body is bounded in memory; the client still receives every byte.

The no-body paths (HEAD/204/304 or `Content-Length: 0`) are recorded in one insert.

## Storage

SQLite in WAL mode. One short-lived connection per operation avoids shared-connection locking and keeps the code simple; WAL allows readers and writers to coexist. Bodies are BLOBs plus a lossy text column for substring search. Headers/trailers/tags are JSON arrays preserving duplicate header names.

Search builds a parameterized `WHERE` clause with `LIKE ... ESCAPE '\'`; all column names are fixed. Status filters support exact codes, `2xx` classes and `error`. `body_contains` can be restricted to request/response/both. Pagination is limited to 500 rows for UI/MCP search; export can request up to 20,000.

Native export is a JSON envelope with base64 bodies, so binary data round-trips. HAR 1.2 export is provided for interoperability. Import accepts the native envelope, a bare array, or HAR; on id conflict a fresh UUIDv7 is assigned.

## Decode model

Decoders take the stored flow, direction, headers, trailers, raw bytes and an options object. They return:

```json
{"format":"grpc","detected":"grpc","summary":"...","text":"...","tree":{...},"meta":{...}}
```

`auto` uses content-type first (gRPC, JSON, XML, form, multipart, SSE, protobuf), then content-encoding decompression, then content sniffing (JSON/XML/text), then a gRPC frame heuristic, then a raw hex dump. This is deliberately conservative: the user can always force a format.

gRPC parsing handles the 5-byte frame header, compressed flags, gRPC-Web base64 and trailer frames, and per-message compression. Protobuf is schema-less: field number/wire type, varints, fixed32/64, length-delimited strings, recursively plausible nested messages, packed varints and hex. It never claims schema knowledge; `meta.schema` is null.

## MCP

MCP is JSON-RPC 2.0 over HTTP POST at `/mcp`. `initialize`, `tools/list`, `tools/call`, `resources/list`, `resources/read`, `ping` and notifications are supported. Tool results return both `content[].text` (pretty JSON) and `structuredContent` for clients that understand it. `traffic_recent` is a convenience view over the same paginated store query as `traffic_search`; both cap a page at 500 rows and expose `next_offset`.

Tool handlers are synchronous wrappers around async store/control operations because the MCP JSON-RPC dispatcher is synchronous. `block_in_place` is used on the Tokio multi-thread runtime; the handler itself must not be called from a current-thread runtime (covered by using a multi-thread server).

The stdio bridge (`mcp-stdio`) forwards newline-delimited JSON-RPC to the HTTP endpoint and prints responses, preserving `Mcp-Session-Id` when present. It never accesses the database directly, so it works with a server started with a custom data dir/port.

## UI embedding

Vite builds `web/dist`. `rust-embed` includes the folder at compile time. `api::static_handler` serves exact files, falls back to `index.html` for SPA routes, and returns a clear message if the build is missing. Hashed assets get immutable caching; HTML gets `no-cache`.

## Lifecycle

`ProxyRuntime` owns the listener task, cancellation token and state. Stop cancels the token and aborts the accept task; its `JoinSet` drops all connection tasks, which close sockets. Start binds before flipping state to `running`, so a port conflict leaves an actionable `error` state. Pause keeps connections but rejects new requests with 503. Port changes persist to `config.json` and restart the listener. WinDivert is stopped with the proxy and restarted after a port change if it was active.

## Deliberate non-goals

- Full TCP/IP stack or UDP proxying.
- HTTP/3/QUIC decapsulation.
- WebSocket message forwarding/capture (raw CONNECT tunnels still work).
- Client certificate authentication and upstream proxy chaining.
- Multi-user auth: bind to loopback by default; if exposed, put it behind a trusted reverse proxy/firewall.
