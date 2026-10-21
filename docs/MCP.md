# MCP guide

MitmRs exposes a **Streamable HTTP** MCP endpoint at:

```
http://127.0.0.1:8787/mcp
```

It speaks JSON-RPC 2.0. POST a single request or a batch:

```powershell
$body = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"my-agent","version":"1.0"}}}'
curl.exe -sS http://127.0.0.1:8787/mcp -H "content-type: application/json" -d $body
```

A successful initialize returns `mcp-session-id`; clients may send it back on subsequent requests. MitmRs does not require the session header today.

## Client setup

### Claude Code

```powershell
claude mcp add --transport http mitmrs http://127.0.0.1:8787/mcp
```

### Cursor / generic HTTP MCP

```json
{
  "mcpServers": {
    "mitmrs": {
      "url": "http://127.0.0.1:8787/mcp"
    }
  }
}
```

### stdio-only clients

```json
{
  "mcpServers": {
    "mitmrs": {
      "command": "F:\\path\\to\\mitmrs.exe",
      "args": ["mcp-stdio", "--url", "http://127.0.0.1:8787/mcp"]
    }
  }
}
```

The bridge is a thin newline-delimited JSON-RPC forwarder; it prints exactly the server's response objects on stdout and diagnostics on stderr.

## Resources

| URI | Content |
|---|---|
| `mitmrs://status` | proxy/store/CA status JSON |
| `mitmrs://ca` | CA certificate PEM |
| `mitmrs://help` | machine-readable workflow guide |

## Tools

### `traffic_recent`

The quick “show me recent traffic” tool: newest first, no query required.

```json
{
  "name": "traffic_recent",
  "arguments": {"limit": 50, "offset": 0}
}
```

Optional `host`, `method`, `status` and `protocol` filters are accepted. The response is `{items,total,limit,offset,next_offset}`; request the next page with `offset=next_offset`. A single call returns at most 500 rows.

### `traffic_search`

Filter summaries. No bodies are returned, which keeps responses small. The same pagination rules apply (`total`, `next_offset`, max 500 rows per call).

```json
{
  "name": "traffic_search",
  "arguments": {
    "host": "api.openai.com",
    "status": "4xx",
    "protocol": "http2",
    "q": "chat",
    "body_contains": "model",
    "direction": "request",
    "limit": 20,
    "order": "desc"
  }
}
```

Response: `{items, total, limit, offset, next_offset}`. Each item has id, seq, time, method/host/path, status, sizes, protocol, MIME and error.

### `traffic_get`

```json
{"name":"traffic_get","arguments":{"id":"<flow-id>","body":"text","decode":true,"format":"auto"}}
```

`body` is `text`, `base64`, `both` or `none`. `decode=true` adds auto-decoded request/response bodies.

### `traffic_body`

```json
{"name":"traffic_body","arguments":{"id":"<flow-id>","direction":"request","format":"base64","max_bytes":2000000}}
```

Use `base64` when exact bytes are needed; `text` for charset-aware view (without decompression); any decoder format otherwise.

### `traffic_decode`

The main “try another packet format” tool.

```json
{"name":"traffic_decode","arguments":{
  "id":"<flow-id>",
  "direction":"response",
  "format":"grpc",
  "options":{"grpc_encoding":"gzip","max_bytes":1000000}
}}
```

Returns `{format,detected,summary,text,tree,meta}`. For gRPC, `tree.frames[]` contains message frames with `protobuf` arrays. For protobuf, `tree.fields[]` contains field number, wire type, string/nested/packed values.

### `traffic_stats`

No arguments. Returns total, top hosts, status class counts, request/response bytes and the first/last timestamps.

### `traffic_export`

```json
{"name":"traffic_export","arguments":{"format":"json","host":"example.com","max_flows":200}}
```

`format` is `json` (MitmRs native, base64 bodies) or `har`. The serialized document is in `data`. Keep `max_flows` modest for chat contexts.

### `traffic_import`

```json
{"name":"traffic_import","arguments":{"data":"<native JSON / array / HAR text>"}}
```

Returns `{ok, imported, skipped}`.

### `traffic_clear` / `traffic_delete`

`traffic_clear` requires `{"confirm":true}`. `traffic_delete` takes an id.

### `proxy_control`

```json
{"action":"status"}
{"action":"pause"}
{"action":"resume"}
{"action":"stop"}
{"action":"start"}
{"action":"restart"}
{"action":"set_config","port":8888,"max_body_bytes":5242880,"capture_enabled":true}
```

Port changes restart the listener and, if enabled, WinDivert. Returns the full status object.

### `ca_certificate`

Returns PEM, SHA-256 fingerprint, paths, validity and the Windows `certutil` command. Never returns the private key.

### `mitmrs_help`

Returns endpoint, proxy address, workflow, decoder formats and tips.

## Agent workflow

1. `ca_certificate` — ensure the CA is trusted (human action if not).
2. `proxy_control {action:"status"}` — if stopped, `start`.
3. Generate traffic through the configured proxy (human/client setup).
4. `traffic_stats` — identify hosts/status distribution.
5. `traffic_search` — narrow by host/path/method/status/body substring.
6. `traffic_get` — inspect headers, trailers and bodies.
7. `traffic_decode` — switch formats (`grpc`, `protobuf`, `json`, `http`, `base64`, ...).
8. `traffic_export` — hand off evidence or feed another tool.

`traffic_recent` is the shortcut when the user says “show me the latest requests” or “what was captured recently”. To enumerate every captured flow, loop `traffic_recent` (or `traffic_search`) using `next_offset` until it is `null`; `total` tells you the expected count.

## Error semantics

JSON-RPC errors use standard codes (`-32700` parse, `-32601` method not found, `-32602` invalid params). Tool execution failures are returned as a successful `tools/call` result with `isError: true` and a human-readable text message, which lets agents inspect and retry.

## Safety

- The default bind is `127.0.0.1`. There is no authentication on `/mcp`; exposing it to a network requires a firewall/reverse proxy.
- `traffic_clear` is destructive but only affects captured rows, not the CA or config.
- Decoding is local; no captured bytes are sent to any remote service.
