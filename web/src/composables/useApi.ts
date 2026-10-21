import type {
  CaInfo,
  DecodeResult,
  DecoderInfo,
  FlowDetail,
  FlowFilters,
  FlowListResponse,
  ImportResult,
  McpInfo,
  ProxyAction,
  ProxyConfigPatch,
  Stats,
  Status,
} from '../types'
import { downloadBlob, filenameFromDisposition } from '../lib/format'

export class ApiError extends Error {
  readonly status: number
  readonly payload: string

  constructor(status: number, message: string, payload = '') {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.payload = payload
  }
}

interface RequestOptions extends RequestInit {
  expectJson?: boolean
}

async function request<T>(
  path: string,
  { expectJson = true, ...init }: RequestOptions = {},
): Promise<T> {
  let response: Response
  try {
    response = await fetch(path, {
      credentials: 'same-origin',
      ...init,
      headers: {
        Accept: 'application/json',
        ...(init.body ? { 'Content-Type': 'application/json' } : {}),
        ...init.headers,
      },
    })
  } catch (error) {
    if (error instanceof DOMException && error.name === 'AbortError') throw error
    throw new ApiError(
      0,
      'Cannot reach the MitmRs backend. Is the server running?',
      error instanceof Error ? error.message : String(error),
    )
  }

  if (!response.ok) {
    const payload = await response.text().catch(() => '')
    let message = `Request failed with HTTP ${response.status}`
    if (payload) {
      try {
        const parsed = JSON.parse(payload) as {
          error?: string
          message?: string
          detail?: string
        }
        message = parsed.error || parsed.message || parsed.detail || message
      } catch {
        if (payload.length < 300) message = payload
      }
    }
    throw new ApiError(response.status, message, payload)
  }

  if (response.status === 204) return undefined as T
  if (!expectJson) return (await response.text()) as T
  const text = await response.text()
  if (!text) return undefined as T
  const type = response.headers.get('content-type') || ''
  if (type.includes('json')) {
    try {
      return JSON.parse(text) as T
    } catch {
      throw new ApiError(0, 'The backend returned an invalid JSON response.', text)
    }
  }
  return text as T
}

export function searchParams(filters: FlowFilters): URLSearchParams {
  const params = new URLSearchParams()
  const optional: Array<[keyof FlowFilters, string]> = [
    ['q', 'q'],
    ['host', 'host'],
    ['method', 'method'],
    ['status', 'status'],
    ['protocol', 'protocol'],
    ['mime', 'mime'],
    ['body_contains', 'body_contains'],
    ['direction', 'direction'],
  ]
  for (const [key, param] of optional) {
    const value = filters[key]
    if (typeof value === 'string' && value.trim()) {
      params.set(param, value.trim())
    }
  }
  if (!params.has('direction')) params.set('direction', 'both')
  return params
}

export function buildFlowQuery(
  filters: FlowFilters,
  limit = 500,
  offset = 0,
): string {
  const params = searchParams(filters)
  params.set('limit', String(limit))
  params.set('offset', String(offset))
  params.set('order', 'desc')
  return params.toString()
}

export const api = {
  getStatus(signal?: AbortSignal): Promise<Status> {
    return request<Status>('/api/status', { signal })
  },

  getFlows(
    filters: FlowFilters,
    limit = 500,
    offset = 0,
    signal?: AbortSignal,
  ): Promise<FlowListResponse> {
    return request<FlowListResponse>(
      `/api/flows?${buildFlowQuery(filters, limit, offset)}`,
      { signal },
    )
  },

  getFlow(
    id: string,
    body: 'text' | 'base64' | 'none' = 'text',
    maxBytes = 1024 * 1024,
    signal?: AbortSignal,
  ): Promise<FlowDetail> {
    const params = new URLSearchParams({
      body,
      max_bytes: String(maxBytes),
    })
    return request<FlowDetail>(
      `/api/flows/${encodeURIComponent(id)}?${params.toString()}`,
      { signal },
    )
  },

  deleteFlow(id: string): Promise<{ ok: boolean }> {
    return request<{ ok: boolean }>(`/api/flows/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    })
  },

  clearFlows(): Promise<{ ok: boolean; deleted: number }> {
    return request<{ ok: boolean; deleted: number }>('/api/flows', {
      method: 'DELETE',
    })
  },

  proxyAction(action: ProxyAction): Promise<Status> {
    return request<Status>(`/api/proxy/${action}`, { method: 'POST' })
  },

  updateConfig(patch: ProxyConfigPatch): Promise<Status> {
    return request<Status>('/api/proxy/config', {
      method: 'POST',
      body: JSON.stringify(patch),
    })
  },

  windivertStart(): Promise<Status> {
    return request<Status>('/api/windivert/start', { method: 'POST' })
  },

  windivertStop(): Promise<Status> {
    return request<Status>('/api/windivert/stop', { method: 'POST' })
  },

  getStats(signal?: AbortSignal): Promise<Stats> {
    return request<Stats>('/api/stats', { signal })
  },

  getDecoders(signal?: AbortSignal): Promise<{ items: DecoderInfo[] }> {
    return request<{ items: DecoderInfo[] }>('/api/decoders', { signal })
  },

  decodeFlow(
    id: string,
    direction: 'request' | 'response',
    format: string,
    options: Record<string, unknown> = {},
    signal?: AbortSignal,
  ): Promise<DecodeResult> {
    return request<DecodeResult>(
      `/api/flows/${encodeURIComponent(id)}/decode`,
      {
        method: 'POST',
        signal,
        body: JSON.stringify({ direction, format, options }),
      },
    )
  },

  getCa(signal?: AbortSignal): Promise<CaInfo> {
    return request<CaInfo>('/api/ca', { signal })
  },

  getMcpInfo(signal?: AbortSignal): Promise<McpInfo> {
    return request<McpInfo>('/api/mcp/info', { signal })
  },

  importFlows(raw: string): Promise<ImportResult> {
    return request<ImportResult>('/api/import', {
      method: 'POST',
      body: raw,
    })
  },

  async downloadExport(
    format: 'json' | 'har',
    filters: FlowFilters,
  ): Promise<string> {
    const params = searchParams(filters)
    params.set('format', format)
    const response = await fetch(`/api/export?${params.toString()}`, {
      credentials: 'same-origin',
      headers: { Accept: format === 'har' ? 'application/json' : '*/*' },
    }).catch((error: unknown) => {
      if (error instanceof DOMException && error.name === 'AbortError') throw error
      throw new ApiError(
        0,
        'Cannot reach the MitmRs backend to export traffic.',
      )
    })
    if (!response.ok) {
      const payload = await response.text().catch(() => '')
      let message = `Export failed with HTTP ${response.status}`
      try {
        const parsed = JSON.parse(payload) as { error?: string; message?: string }
        message = parsed.error || parsed.message || message
      } catch {
        if (payload && payload.length < 300) message = payload
      }
      throw new ApiError(response.status, message, payload)
    }
    const blob = await response.blob()
    const fallback = `mitmrs-flows-${Date.now()}.${format === 'har' ? 'har' : 'json'}`
    const filename = filenameFromDisposition(
      response.headers.get('content-disposition'),
      fallback,
    )
    downloadBlob(blob, filename)
    return filename
  },

  async downloadCa(): Promise<string> {
    const response = await fetch('/api/ca.crt', {
      credentials: 'same-origin',
    }).catch(() => {
      throw new ApiError(0, 'Cannot reach the MitmRs backend to download the CA.')
    })
    if (!response.ok) {
      throw new ApiError(
        response.status,
        `CA download failed with HTTP ${response.status}`,
      )
    }
    const blob = await response.blob()
    const filename = filenameFromDisposition(
      response.headers.get('content-disposition'),
      'mitmrs-ca.crt',
    )
    downloadBlob(blob, filename)
    return filename
  },
}

export function isAbortError(error: unknown): boolean {
  return error instanceof DOMException && error.name === 'AbortError'
}

export function errorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    return error.status ? `${error.message} (HTTP ${error.status})` : error.message
  }
  if (error instanceof Error) return error.message
  return String(error)
}
