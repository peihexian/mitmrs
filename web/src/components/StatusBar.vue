<script setup lang="ts">
import { computed } from 'vue'
import AppIcon from './AppIcon.vue'
import type { Status } from '../types'
import type { LiveTransport } from '../composables/useLive'
import { formatClockMs, formatNumber, proxyAddress } from '../lib/format'

const props = defineProps<{
  status: Status | null
  live: boolean
  transport: LiveTransport
  lastEventAt: Date | string | null
  totalFlows: number
}>()

const emit = defineEmits<{ 'open-mcp': [] }>()

const proxyEndpoint = computed(() =>
  props.status
    ? proxyAddress(props.status.proxy.host, props.status.proxy.port)
    : '—',
)
const mcpEndpoint = computed(
  () => props.status?.mcp.endpoint || '/mcp',
)
const lastEvent = computed(() => {
  const value = props.lastEventAt
  if (!value) return '—'
  return formatClockMs(value instanceof Date ? value.toISOString() : value)
})
const connectionLabel = computed(() => {
  if (!props.live) return 'paused'
  if (props.transport === 'sse') return 'sse'
  if (props.transport === 'polling') return 'polling'
  return 'connecting'
})
</script>

<template>
  <footer
    class="relative z-20 flex h-7 shrink-0 items-center gap-0 overflow-x-auto border-t border-ink-700/80 bg-ink-925/95 px-2 text-[9.5px] text-mist-600 [scrollbar-width:none] sm:px-3"
  >
    <div class="flex shrink-0 items-center gap-1.5 pr-2.5" :title="`Proxy listener ${proxyEndpoint}`">
      <span
        class="h-1.5 w-1.5 rounded-full"
        :class="
          status?.proxy.state === 'running'
            ? 'bg-emerald-400'
            : status?.proxy.state === 'paused'
              ? 'bg-amber-400'
              : status?.proxy.state === 'error'
                ? 'bg-rose-400'
                : 'bg-mist-700'
        "
      />
      <AppIcon name="plug" :size="10" class="text-mist-700" />
      <span class="font-mono text-[9px] text-mist-500">{{ proxyEndpoint }}</span>
    </div>
    <span class="h-3 w-px shrink-0 bg-ink-700/70" />
    <button
      class="flex shrink-0 items-center gap-1.5 px-2.5 py-1 transition hover:text-accent-300"
      type="button"
      title="Open MCP integration"
      @click="emit('open-mcp')"
    >
      <AppIcon name="sparkles" :size="9" class="text-accent-500/70" />
      <span class="text-[9px]">MCP</span>
      <span class="mono max-w-[180px] truncate text-[8.5px] text-mist-700">{{ mcpEndpoint }}</span>
    </button>
    <span class="h-3 w-px shrink-0 bg-ink-700/70" />
    <div class="flex shrink-0 items-center gap-1.5 px-2.5" title="Total captured flows in the store">
      <AppIcon name="database" :size="9" class="text-mist-700" />
      <span class="mono text-[9px] tabular-nums text-mist-600">{{ formatNumber(totalFlows) }}</span>
      <span class="hidden sm:inline">flows</span>
    </div>
    <div class="ml-auto flex shrink-0 items-center gap-2 pl-2">
      <div class="hidden items-center gap-1.5 md:flex" :title="`Event stream: ${connectionLabel}`">
        <span
          class="h-1 w-1 rounded-full"
          :class="
            live && transport === 'sse'
              ? 'bg-emerald-400 animate-pulse-dot'
              : live && transport === 'polling'
                ? 'bg-amber-400'
                : 'bg-mist-700'
          "
        />
        <span class="text-[8.5px] text-mist-700">{{ lastEvent === '—' ? 'no events' : lastEvent }}</span>
      </div>
      <span class="hidden h-3 w-px bg-ink-700/70 md:block" />
      <span class="flex items-center gap-1 text-[8.5px] text-mist-700">
        <AppIcon name="cpu" :size="9" />
        <span class="font-mono">v{{ status?.version || '—' }}</span>
      </span>
    </div>
  </footer>
</template>
