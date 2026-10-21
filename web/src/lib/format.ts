import type { BodyView, Direction, FlowFilters, FlowSummary } from '../types'

const pad = (value: number) => String(value).padStart(2, '0')

export function formatClock(value: string | null | undefined): string {
  if (!value) return '—'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return '—'
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

export function formatClockMs(value: string | null | undefined): string {
  if (!value) return '—'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return '—'
  return `${formatClock(value)}.${String(date.getMilliseconds()).padStart(3, '0')}`
}

export function formatDateTime(value: string | null | undefined): string {
  if (!value) return '—'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return '—'
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

export function formatCalendarDate(value: string | null | undefined): string {
  if (!value) return '—'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return '—'
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return '—'
  if (bytes === 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const exponent = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1,
  )
  const value = bytes / 1024 ** exponent
  if (exponent === 0) return `${Math.round(value)} B`
  return `${value >= 100 ? value.toFixed(0) : value >= 10 ? value.toFixed(1) : value.toFixed(2)} ${units[exponent]}`
}

export function formatDuration(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return '—'
  if (ms < 1) return '<1 ms'
  if (ms < 1000) return `${Math.round(ms)} ms`
  if (ms < 60_000) {
    return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`
  }
  const minutes = Math.floor(ms / 60_000)
  const seconds = Math.round((ms % 60_000) / 1000)
  return `${minutes}m ${String(seconds).padStart(2, '0')}s`
}

export function formatNumber(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return '—'
  return new Intl.NumberFormat('en-US').format(value)
}

export function statusCodeLabel(
  status: number | null,
  error: string | null,
): string {
  if (status != null) return String(status)
  if (error) return 'ERR'
  return '···'
}

export function statusTone(
  status: number | null,
  error: string | null,
): string {
  if (error || (status != null && status >= 500)) return 'text-rose-400'
  if (status == null) return 'text-mist-600'
  if (status >= 400) return 'text-amber-400'
  if (status >= 300) return 'text-sky-400'
  if (status >= 200) return 'text-emerald-400'
  return 'text-mist-500'
}

export function statusDotTone(
  status: number | null,
  error: string | null,
): string {
  if (error || (status != null && status >= 500)) return 'bg-rose-400'
  if (status == null) return 'bg-mist-700'
  if (status >= 400) return 'bg-amber-400'
  if (status >= 300) return 'bg-sky-400'
  if (status >= 200) return 'bg-emerald-400'
  return 'bg-mist-600'
}

export function methodTone(method: string): string {
  switch ((method || '').toUpperCase()) {
    case 'GET':
      return 'text-cyan-300'
    case 'POST':
      return 'text-amber-300'
    case 'PUT':
      return 'text-orange-300'
    case 'PATCH':
      return 'text-yellow-300'
    case 'DELETE':
      return 'text-rose-300'
    case 'HEAD':
      return 'text-teal-300'
    case 'OPTIONS':
      return 'text-sky-300'
    case 'CONNECT':
      return 'text-mist-400'
    default:
      return 'text-mist-300'
  }
}

export function protocolLabel(protocol: string): string {
  const value = (protocol || '').toLowerCase().trim()
  if (value === 'grpc') return 'gRPC'
  if (value === 'http/2' || value === 'h2' || value === 'http2') return 'H2'
  if (value === 'http/1.1' || value === 'http1.1' || value === 'h1')
    return 'H1'
  if (value === 'http/3' || value === 'h3') return 'H3'
  if (value === 'websocket' || value === 'ws' || value === 'wss') return 'WS'
  if (!value) return '—'
  return protocol.length > 8 ? protocol.slice(0, 8) : protocol
}

export function fullProtocolLabel(protocol: string): string {
  const value = (protocol || '').toLowerCase().trim()
  if (value === 'grpc') return 'gRPC'
  if (value === 'http/2' || value === 'h2' || value === 'http2') return 'HTTP/2'
  if (value === 'http/1.1' || value === 'http1.1' || value === 'h1')
    return 'HTTP/1.1'
  if (value === 'websocket' || value === 'ws' || value === 'wss') return 'WebSocket'
  if (!value) return 'Unknown'
  return protocol
}

export function isGrpc(flow: Pick<FlowSummary, 'protocol' | 'request_mime' | 'response_mime' | 'http_version'>): boolean {
  return (
    (flow.protocol || '').toLowerCase().includes('grpc') ||
    (flow.request_mime || '').toLowerCase().includes('grpc') ||
    (flow.response_mime || '').toLowerCase().includes('grpc')
  )
}

export function isWebSocket(
  flow: Pick<FlowSummary, 'protocol' | 'http_version' | 'status'>,
): boolean {
  return (
    (flow.protocol || '').toLowerCase().includes('websocket') ||
    (flow.http_version || '').toLowerCase().includes('websocket') ||
    flow.status === 101
  )
}

export function flowUrl(flow: Pick<FlowSummary, 'scheme' | 'host' | 'port' | 'path'>): string {
  const scheme = flow.scheme || 'https'
  const host = flow.host || 'unknown'
  const defaultPort =
    (scheme === 'https' && flow.port === 443) ||
    (scheme === 'http' && flow.port === 80)
  const port = flow.port && !defaultPort ? `:${flow.port}` : ''
  const path = flow.path || '/'
  return `${scheme}://${host}${port}${path.startsWith('/') ? path : `/${path}`}`
}

export function proxyAddress(host: string, port: number): string {
  const targetHost = !host || host === '0.0.0.0' || host === '::' ? '127.0.0.1' : host
  return `${targetHost}:${port}`
}

export function base64ToBytes(input: string): Uint8Array {
  const clean = input.replace(/\s+/g, '')
  const binary = atob(clean)
  const bytes = new Uint8Array(binary.length)
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index)
  }
  return bytes
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = ''
  const chunk = 0x8000
  for (let offset = 0; offset < bytes.length; offset += chunk) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunk))
  }
  return btoa(binary)
}

export function bodyBytes(view: BodyView | null | undefined): Uint8Array {
  if (!view) return new Uint8Array()
  if (view.body_base64) {
    try {
      return base64ToBytes(view.body_base64)
    } catch {
      // Fall through to the text payload.
    }
  }
  if (view.body_text != null) return new TextEncoder().encode(view.body_text)
  return new Uint8Array()
}

export function bodyText(
  view: BodyView | null | undefined,
  maxBytes = Number.POSITIVE_INFINITY,
): string | null {
  if (!view) return null
  if (view.body_text != null) {
    return view.body_text.length > maxBytes
      ? view.body_text.slice(0, maxBytes)
      : view.body_text
  }
  if (!view.body_base64) return null
  try {
    const bytes = base64ToBytes(view.body_base64)
    const sliced = bytes.slice(0, Math.min(bytes.length, maxBytes))
    return new TextDecoder('utf-8', { fatal: false }).decode(sliced)
  } catch {
    return null
  }
}

export function looksBinary(bytes: Uint8Array): boolean {
  if (bytes.length === 0) return false
  const limit = Math.min(bytes.length, 4096)
  let suspicious = 0
  for (let index = 0; index < limit; index += 1) {
    const value = bytes[index]
    if (value === 0) return true
    if (value < 9 || (value > 13 && value < 32)) suspicious += 1
  }
  return suspicious / limit > 0.08
}

export function hexDump(bytes: Uint8Array, maxBytes = 256 * 1024): string {
  const length = Math.min(bytes.length, maxBytes)
  const lines: string[] = []
  for (let offset = 0; offset < length; offset += 16) {
    const slice = bytes.subarray(offset, Math.min(offset + 16, length))
    const hex = Array.from(slice)
      .map((byte) => byte.toString(16).padStart(2, '0'))
      .join(' ')
      .padEnd(47, ' ')
    const ascii = Array.from(slice)
      .map((byte) => (byte >= 32 && byte <= 126 ? String.fromCharCode(byte) : '.'))
      .join('')
    lines.push(
      `${offset.toString(16).padStart(8, '0')}  ${hex}  |${ascii.padEnd(16, ' ')}|`,
    )
  }
  if (bytes.length > length) {
    lines.push(`\n… ${formatBytes(bytes.length - length)} not shown`)
  }
  return lines.join('\n')
}

export function prettyJson(text: string): string | null {
  try {
    return JSON.stringify(JSON.parse(text), null, 2)
  } catch {
    return null
  }
}

function parseXml(text: string): Document | null {
  try {
    const doc = new DOMParser().parseFromString(text, 'application/xml')
    if (doc.getElementsByTagName('parsererror').length > 0) return null
    return doc
  } catch {
    return null
  }
}

function serializeXml(node: Node, depth: number): string {
  const indent = '  '.repeat(depth)
  if (node.nodeType === Node.TEXT_NODE) {
    const value = node.nodeValue?.trim() ?? ''
    return value ? indent + value : ''
  }
  if (node.nodeType === Node.COMMENT_NODE) {
    return `${indent}<!--${node.nodeValue ?? ''}-->`
  }
  if (node.nodeType === Node.CDATA_SECTION_NODE) {
    return `${indent}<![CDATA[${node.nodeValue ?? ''}]]>`
  }
  if (node.nodeType !== Node.ELEMENT_NODE) return ''
  const element = node as Element
  const attrs = Array.from(element.attributes)
    .map((attr) => `${attr.name}="${attr.value}"`)
    .join(' ')
  const open = `<${element.tagName}${attrs ? ` ${attrs}` : ''}`
  const children = Array.from(element.childNodes).filter(
    (child) =>
      child.nodeType === Node.ELEMENT_NODE ||
      (child.nodeType === Node.TEXT_NODE && Boolean(child.nodeValue?.trim())),
  )
  const textOnly =
    children.length === 0 ||
    children.every((child) => child.nodeType === Node.TEXT_NODE)
  if (textOnly) {
    const text = (element.textContent ?? '').trim()
    return `${indent}${open}>${escapeXmlText(text)}</${element.tagName}>`
  }
  const rendered = children
    .map((child) => serializeXml(child, depth + 1))
    .filter(Boolean)
    .join('\n')
  return `${indent}${open}>\n${rendered}\n${indent}</${element.tagName}>`
}

function escapeXmlText(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
}

export function prettyXml(text: string): string | null {
  const doc = parseXml(text)
  if (!doc || !doc.documentElement) return null
  try {
    return serializeXml(doc.documentElement, 0)
  } catch {
    return null
  }
}

export function prettyBody(
  text: string,
  contentType: string | null,
): { text: string; language: string } {
  const type = (contentType || '').toLowerCase()
  const trimmed = text.trimStart()
  if (type.includes('json') || trimmed.startsWith('{') || trimmed.startsWith('[')) {
    const json = prettyJson(text)
    if (json != null) return { text: json, language: 'JSON' }
  }
  if (
    type.includes('xml') ||
    type.includes('html') ||
    trimmed.startsWith('<?xml') ||
    trimmed.startsWith('<')
  ) {
    const xml = prettyXml(text)
    if (xml != null) return { text: xml, language: type.includes('html') ? 'HTML' : 'XML' }
  }
  return { text, language: 'Text' }
}

export function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

export function headersToText(
  headers: { name: string; value: string }[],
  trailers: { name: string; value: string }[] = [],
): string {
  const lines = headers.map((header) => `${header.name}: ${header.value}`)
  if (trailers.length > 0) {
    lines.push('', '[trailers]')
    lines.push(...trailers.map((header) => `${header.name}: ${header.value}`))
  }
  return lines.join('\n')
}

export function jsonStringify(value: unknown): string {
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

export function directionLabel(direction: Direction): string {
  if (direction === 'request') return 'Request'
  if (direction === 'response') return 'Response'
  return 'Both directions'
}

export function activeFilterCount(filters: FlowFilters): number {
  let count = 0
  if (filters.q.trim()) count += 1
  if (filters.host) count += 1
  if (filters.method) count += 1
  if (filters.status) count += 1
  if (filters.protocol) count += 1
  if (filters.mime.trim()) count += 1
  if (filters.body_contains.trim()) count += 1
  if (filters.direction !== 'both') count += 1
  return count
}

export function matchesFilters(flow: FlowSummary, filters: FlowFilters): boolean {
  if (filters.host && flow.host !== filters.host) return false
  if (filters.method && flow.method.toUpperCase() !== filters.method.toUpperCase())
    return false
  if (filters.protocol) {
    const protocol = (flow.protocol || '').toLowerCase()
    const wanted = filters.protocol.toLowerCase()
    if (wanted === 'other') {
      if (
        ['http/1.1', 'http/2', 'grpc', 'websocket'].some((item) =>
          protocol.includes(item),
        )
      ) {
        return false
      }
    } else if (!protocol.includes(wanted)) {
      return false
    }
  }
  if (filters.mime) {
    const mime = `${flow.request_mime || ''} ${flow.response_mime || ''}`.toLowerCase()
    if (!mime.includes(filters.mime.toLowerCase())) return false
  }
  if (filters.status === 'error') {
    if (!flow.error) return false
  } else if (filters.status) {
    const wanted = filters.status.replace('xx', '')
    if (flow.status == null) return false
    if (String(Math.floor(flow.status / 100)) !== wanted) return false
  }
  if (filters.body_contains) return true
  const q = filters.q.trim().toLowerCase()
  if (q) {
    const haystack = `${flow.method} ${flow.host} ${flow.path} ${flow.protocol} ${flow.status ?? ''} ${flow.client}`.toLowerCase()
    if (!haystack.includes(q)) return false
  }
  return true
}

export function filenameFromDisposition(
  header: string | null,
  fallback: string,
): string {
  if (!header) return fallback
  const star = /filename\*\s*=\s*(?:UTF-8'')?([^;]+)/i.exec(header)
  if (star?.[1]) {
    try {
      return decodeURIComponent(star[1].trim().replace(/^"|"$/g, ''))
    } catch {
      return star[1].trim().replace(/^"|"$/g, '')
    }
  }
  const plain = /filename\s*=\s*"?([^";]+)"?/i.exec(header)
  return plain?.[1]?.trim() || fallback
}

export async function copyText(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text)
    return
  }
  const area = document.createElement('textarea')
  area.value = text
  area.style.position = 'fixed'
  area.style.opacity = '0'
  document.body.appendChild(area)
  area.select()
  document.execCommand('copy')
  area.remove()
}

export function downloadBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = filename
  document.body.appendChild(anchor)
  anchor.click()
  anchor.remove()
  window.setTimeout(() => URL.revokeObjectURL(url), 2000)
}

export function downloadText(
  text: string,
  filename: string,
  type = 'text/plain;charset=utf-8',
): void {
  downloadBlob(new Blob([text], { type }), filename)
}
