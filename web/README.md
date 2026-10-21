# MitmRs Web

The MitmRs web UI: an AI-native MITM traffic lab built with Vue 3, TypeScript,
Vite, and Tailwind CSS v4. It is served by the Rust backend from the site root
and embedded from `web/dist` with `rust-embed`.

## Commands

```bash
pnpm install
pnpm dev        # Vite dev server at http://127.0.0.1:5173
pnpm build      # Type-check with vue-tsc, then build dist/
pnpm preview    # Preview the production build on port 4173
pnpm typecheck  # vue-tsc --noEmit
```

In development, `/api` and `/mcp` are proxied to the local Rust backend. The
default target is the Rust web/API listener at `http://127.0.0.1:8787` (the
default proxy capture listener is separately `127.0.0.1:8080`). Override it
when the backend web port differs:

```powershell
$env:MITMRS_BACKEND_URL = "http://127.0.0.1:9000"
pnpm dev
```

The production build uses `base: '/'` and emits absolute `/assets/...` URLs so
the Rust server can serve it directly at the site root. `dist/` is generated —
do not edit it by hand.

`.npmrc` enables pnpm's portable shell emulator. This keeps `pnpm build`
consistent on Windows when a host-level `script-shell` setting interferes with
`&&` in npm scripts.

## What's implemented

- Live traffic table with row virtualization, keyboard navigation, gRPC/error
  badges, status classes, MIME/protocol/host/search filters, and live
  SSE updates with polling fallback.
- Detail workspace with request/response bodies (pretty JSON, pretty XML,
  text, raw, hex), headers/trailers, backend decoders (recursive tree,
  gRPC messages, metadata), generated cURL, and selected-flow MCP examples.
- Proxy lifecycle controls, listener port apply, import/export (native JSON and
  HAR), CA download/trust guidance, MCP client setup, settings, and toasts.
- Responsive layout: mobile filter drawer, full-screen flow detail below
  `xl`, condensed table columns, and a dense desktop three-pane layout.
- Search and filter state is mirrored into URL query parameters; `live=0`
  persists the paused live-toggle state.

## API assumptions

All calls are same-origin. The UI follows the backend API contract:

- Status and stats are fetched at startup and refreshed while the page is
  visible; SSE `/api/events` supplies `flow` and `status` events.
- If the EventSource cannot stay connected, the UI polls `/api/flows`,
  `/api/stats`, and `/api/status` every three seconds while live updates are on.
- Filter query values are sent as `q`, `host`, `method`, `status`, `protocol`,
  `mime`, `body_contains`, `direction`, `limit`, `offset`, and `order=desc`.
- The table requests the latest 500 flows. If the server reports a larger
  matching total, the UI states that older flows are hidden by the limit.
- Export requests use the same filter parameters plus `format=json|har`;
  downloads use the `Content-Disposition` filename when provided.
- Optional or evolving fields (extra decoders, CA validity, MCP examples) are
  rendered defensively and hidden when absent.
