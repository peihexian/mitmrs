import { onScopeDispose, ref } from 'vue'
import type { FlowSummary, Status } from '../types'

export type LiveTransport = 'idle' | 'sse' | 'polling'

interface LiveHandlers {
  onFlow?: (flow: FlowSummary) => void
  onStatus?: (status: Status) => void
  onPoll?: () => void
  onPing?: () => void
}

export function useLive(handlers: LiveHandlers) {
  const connected = ref(false)
  const transport = ref<LiveTransport>('idle')
  const lastEventAt = ref<Date | null>(null)
  let source: EventSource | null = null
  let pollTimer: number | null = null
  let reconnectTimer: number | null = null
  let running = false
  let failureCount = 0

  function markEvent(): void {
    lastEventAt.value = new Date()
  }

  function stopPolling(): void {
    if (pollTimer != null) {
      window.clearInterval(pollTimer)
      pollTimer = null
    }
  }

  function startPolling(): void {
    if (!running || pollTimer != null) return
    transport.value = 'polling'
    handlers.onPoll?.()
    pollTimer = window.setInterval(() => {
      if (document.visibilityState !== 'hidden') handlers.onPoll?.()
    }, 3000)
  }

  function closeSource(): void {
    if (source) {
      source.close()
      source = null
    }
  }

  function scheduleReconnect(delay = 5000): void {
    if (!running || reconnectTimer != null) return
    reconnectTimer = window.setTimeout(() => {
      reconnectTimer = null
      if (!running) return
      if (!source || source.readyState === EventSource.CLOSED) {
        connect()
      }
    }, delay)
  }

  function connect(): void {
    if (!running) return
    closeSource()
    try {
      const next = new EventSource('/api/events')
      source = next

      next.onopen = () => {
        failureCount = 0
        connected.value = true
        transport.value = 'sse'
        stopPolling()
      }

      next.addEventListener('flow', (event) => {
        markEvent()
        try {
          const data = JSON.parse((event as MessageEvent<string>).data) as
            | FlowSummary
            | { lagged?: number }
          if (data && 'id' in data && data.id) {
            handlers.onFlow?.(data)
          } else if (data && 'lagged' in data) {
            // The backend tells us the broadcast channel lagged; resync from
            // the REST list rather than leaving a hole in the captured rows.
            handlers.onPoll?.()
          }
        } catch {
          // Ignore malformed events; the next fetch will reconcile state.
        }
      })

      next.addEventListener('status', (event) => {
        markEvent()
        try {
          const data = JSON.parse((event as MessageEvent<string>).data) as Status
          if (data && data.proxy) handlers.onStatus?.(data)
        } catch {
          // Ignore malformed status payloads.
        }
      })

      next.addEventListener('ping', () => {
        markEvent()
        handlers.onPing?.()
      })

      next.onerror = () => {
        connected.value = false
        failureCount += 1
        if (transport.value === 'sse' || transport.value === 'idle') {
          transport.value = 'polling'
        }
        startPolling()
        if (!next || next.readyState === EventSource.CLOSED) {
          scheduleReconnect(Math.min(5000 + failureCount * 1000, 15_000))
        }
      }
    } catch {
      connected.value = false
      transport.value = 'polling'
      startPolling()
      scheduleReconnect(5000)
    }
  }

  function start(): void {
    if (running) return
    running = true
    connect()
  }

  function stop(): void {
    running = false
    closeSource()
    stopPolling()
    if (reconnectTimer != null) {
      window.clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
    connected.value = false
    transport.value = 'idle'
  }

  function reconnect(): void {
    if (!running) {
      start()
      return
    }
    closeSource()
    connect()
  }

  onScopeDispose(stop)

  return {
    connected,
    transport,
    lastEventAt,
    start,
    stop,
    reconnect,
  }
}
