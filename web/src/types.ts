export type ProxyState =
  | 'stopped'
  | 'starting'
  | 'running'
  | 'paused'
  | 'stopping'
  | 'error'

export type Direction = 'both' | 'request' | 'response'

export interface WindivertStatus {
  enabled: boolean
  state: string
  error?: string | null
  packets: number
}

export interface ProxyStatus {
  state: ProxyState
  host: string
  port: number
  error?: string | null
  active_connections: number
  total_connections: number
  windivert: WindivertStatus
}

export interface CaStatus {
  subject: string
  fingerprint: string
  path: string
  pem_url: string
  not_before?: string | null
  not_after?: string | null
}

export interface McpStatus {
  endpoint: string
  stdio_command: string
  tools: string[]
}

export interface BackendConfig {
  max_body_bytes: number
  capture_enabled: boolean
}

export interface StoreStatus {
  flows: number
  bytes: number
  db_path: string
}

export interface Status {
  version: string
  started_at: string
  proxy: ProxyStatus
  store: StoreStatus
  ca: CaStatus
  mcp: McpStatus
  config: BackendConfig
}

export interface FlowSummary {
  id: string
  seq: number
  started_at: string
  duration_ms: number | null
  client: string
  scheme: string
  host: string
  port: number
  method: string
  path: string
  http_version: string
  status: number | null
  request_size: number
  response_size: number
  request_mime: string | null
  response_mime: string | null
  error: string | null
  protocol: string
  tags: string[]
}

export interface HeaderEntry {
  name: string
  value: string
}

export interface BodyView {
  headers: HeaderEntry[]
  trailers: HeaderEntry[]
  body_size: number
  truncated: boolean
  body_base64: string | null
  body_text: string | null
  body_encoding: string | null
  content_type: string | null
}

export interface FlowDetail extends FlowSummary {
  request: BodyView
  response: BodyView
  tls_sni: string | null
  error_detail?: string | null
}

export interface DecodeResult {
  format: string
  detected: string | null
  summary: string
  text: string | null
  tree: unknown
  meta: Record<string, unknown>
}

export interface DecoderOption {
  name: string
  type: string
  description: string
  default?: unknown
}

export interface DecoderInfo {
  id: string
  name: string
  description: string
  content_types: string[]
  options: DecoderOption[]
}

export interface FlowListResponse {
  items: FlowSummary[]
  total: number
  limit: number
  offset: number
}

export interface HostCount {
  host: string
  count: number
}

export interface StatusClassCount {
  class: string
  count: number
}

export interface Stats {
  total: number
  hosts: HostCount[]
  status_classes: StatusClassCount[]
  bytes: {
    request: number
    response: number
  }
  first: string | null
  last: string | null
}

export interface ImportResult {
  ok: boolean
  imported: number
  skipped: number
}

export interface CaInfo {
  subject: string
  fingerprint: string
  path: string
  pem_url: string
  not_before?: string | null
  not_after?: string | null
}

export interface McpTool {
  name: string
  description: string
}

export interface McpInfo {
  endpoint: string
  stdio_command: string
  tools: McpTool[]
  examples?: Record<string, unknown>
}

export interface FlowFilters {
  q: string
  host: string
  method: string
  status: string
  protocol: string
  mime: string
  body_contains: string
  direction: Direction
}

export interface ProxyConfigPatch {
  port?: number
  max_body_bytes?: number
  capture_enabled?: boolean
}

export type ProxyAction = 'start' | 'stop' | 'pause' | 'resume'

export type ToastKind = 'success' | 'error' | 'info'

export interface Toast {
  id: number
  kind: ToastKind
  title: string
  message?: string
  detail?: string
}

export type DetailTab =
  | 'request'
  | 'response'
  | 'headers'
  | 'decode'
  | 'curl'
  | 'mcp'
