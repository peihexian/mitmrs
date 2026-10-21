<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import AppIcon from './AppIcon.vue'
import BodyViewer from './BodyViewer.vue'
import DecodePanel from './DecodePanel.vue'
import CurlPanel from './CurlPanel.vue'
import McpTab from './McpTab.vue'
import type { BodyView, DetailTab, FlowDetail, FlowSummary } from '../types'
import { api, isAbortError } from '../composables/useApi'
import {
  copyText,
  flowUrl,
  formatBytes,
  formatDateTime,
  formatDuration,
  headersToText,
  isGrpc,
  methodTone,
  protocolLabel,
  statusCodeLabel,
  statusDotTone,
  statusTone,
} from '../lib/format'
import { toastError } from '../composables/useToasts'

const props = defineProps<{
  flowId: string | null
  summary: FlowSummary | null
  refreshKey: number
  mcpEndpoint: string
}>()

const emit = defineEmits<{ close: []; delete: [] }>()

const detail = ref<FlowDetail | null>(null)
const loading = ref(false)
const error = ref('')
const activeTab = ref<DetailTab>('request')
const headersCopied = ref<'' | 'request' | 'response'>('')
const urlCopied = ref(false)
let controller: AbortController | null = null
let requestId = 0
let disposed = false
let copiedTimer: number | null = null

const tabItems: Array<{ id: DetailTab; label: string }> = [
  { id: 'request', label: 'Request' },
  { id: 'response', label: 'Response' },
  { id: 'headers', label: 'Headers' },
  { id: 'decode', label: 'Decode' },
  { id: 'curl', label: 'cURL' },
  { id: 'mcp', label: 'MCP' },
]

const flow = computed(() => detail.value ?? props.summary)
const url = computed(() => (flow.value ? flowUrl(flow.value) : ''))
const grpc = computed(() => (flow.value ? isGrpc(flow.value) : false))
const hasError = computed(() => Boolean(flow.value?.error || detail.value?.error_detail))

const requestHeaders = computed(() => detail.value?.request.headers ?? [])
const responseHeaders = computed(() => detail.value?.response.headers ?? [])
const requestTrailers = computed(() => detail.value?.request.trailers ?? [])
const responseTrailers = computed(() => detail.value?.response.trailers ?? [])

async function loadDetail(background = false): Promise<void> {
  if (disposed) return
  if (!props.flowId) {
    detail.value = null
    error.value = ''
    loading.value = false
    return
  }
  const currentRequest = ++requestId
  controller?.abort()
  const nextController = new AbortController()
  controller = nextController
  if (!background) {
    detail.value = null
    error.value = ''
  }
  loading.value = true
  try {
    const result = await api.getFlow(
      props.flowId,
      'text',
      1024 * 1024,
      nextController.signal,
    )
    if (currentRequest === requestId && result && result.id === props.flowId) {
      detail.value = result
      error.value = ''
    }
  } catch (loadError) {
    if (currentRequest === requestId && !isAbortError(loadError)) {
      error.value =
        loadError instanceof Error ? loadError.message : String(loadError)
    }
  } finally {
    if (currentRequest === requestId) {
      loading.value = false
      controller = null
    }
  }
}

function httpVersionLabel(value: string | null | undefined): string {
  if (!value) return 'HTTP/?'
  return /^http\//i.test(value) ? value : `HTTP/${value}`
}

watch(
  () => props.flowId,
  () => {
    activeTab.value = 'request'
    detail.value = null
    error.value = ''
    void loadDetail()
  },
  { immediate: true },
)

watch(
  () => props.refreshKey,
  () => {
    if (props.flowId) void loadDetail(true)
  },
)

function selectTab(tab: DetailTab): void {
  activeTab.value = tab
}

async function copyUrl(): Promise<void> {
  if (!url.value) return
  try {
    await copyText(url.value)
    urlCopied.value = true
    if (copiedTimer != null) window.clearTimeout(copiedTimer)
    copiedTimer = window.setTimeout(() => {
      urlCopied.value = false
    }, 1600)
  } catch (copyError) {
    toastError(copyError, 'Could not copy the URL')
  }
}

async function copyHeaders(view: BodyView, direction: 'request' | 'response'): Promise<void> {
  try {
    await copyText(headersToText(view.headers, view.trailers))
    headersCopied.value = direction
    if (copiedTimer != null) window.clearTimeout(copiedTimer)
    copiedTimer = window.setTimeout(() => {
      headersCopied.value = ''
    }, 1600)
  } catch (copyError) {
    toastError(copyError, 'Could not copy the headers')
  }
}

onBeforeUnmount(() => {
  disposed = true
  controller?.abort()
  if (copiedTimer != null) window.clearTimeout(copiedTimer)
})
</script>

<template>
  <section class="flex h-full min-h-0 flex-col overflow-hidden bg-ink-900/95">
    <template v-if="!flowId || !flow">
      <div class="flex flex-1 flex-col items-center justify-center px-8 text-center">
        <div class="relative flex h-14 w-14 items-center justify-center rounded-2xl border border-ink-700 bg-ink-900">
          <div class="absolute inset-1.5 rounded-xl border border-ink-700/60" />
          <AppIcon name="list" :size="23" class="relative text-mist-600" />
        </div>
        <h2 class="mt-4 text-[13px] font-semibold text-mist-300">Select a flow</h2>
        <p class="mt-1.5 max-w-[270px] text-[11px] leading-relaxed text-mist-600">
          Inspect headers, bodies, wire formats, and AI-ready tool calls for
          any captured request.
        </p>
        <div class="mt-5 grid w-full max-w-[280px] gap-1.5 text-left">
          <div class="flex items-center justify-between rounded-md border border-ink-700/60 bg-ink-950/25 px-2.5 py-1.5">
            <span class="flex items-center gap-1.5 text-[10px] text-mist-500"><AppIcon name="arrow-up" :size="10" /> / <AppIcon name="arrow-down" :size="10" /> navigate</span>
            <span class="font-mono text-[9px] text-mist-700">rows</span>
          </div>
          <div class="flex items-center justify-between rounded-md border border-ink-700/60 bg-ink-950/25 px-2.5 py-1.5">
            <span class="flex items-center gap-1.5 text-[10px] text-mist-500"><AppIcon name="search" :size="10" /> focus search</span>
            <span class="kbd">/</span>
          </div>
          <div class="flex items-center justify-between rounded-md border border-ink-700/60 bg-ink-950/25 px-2.5 py-1.5">
            <span class="flex items-center gap-1.5 text-[10px] text-mist-500"><AppIcon name="sparkles" :size="10" /> MCP hand-off</span>
            <span class="font-mono text-[9px] text-mist-700">per flow</span>
          </div>
        </div>
      </div>
    </template>

    <template v-else>
      <header class="shrink-0 border-b border-ink-700/80 bg-ink-925/70">
        <div class="flex items-center gap-2 px-3 pt-2.5">
          <div class="flex min-w-0 flex-1 items-center gap-1.5">
            <span class="mono rounded border border-ink-700 bg-ink-950/40 px-1 py-px text-[9px] tabular-nums text-mist-600">
              #{{ flow.seq }}
            </span>
            <span class="mono text-[11px] font-bold tracking-tight" :class="methodTone(flow.method)">
              {{ flow.method }}
            </span>
            <span class="h-3 w-px bg-ink-700" />
            <span class="flex items-center gap-1">
              <span class="h-1.5 w-1.5 rounded-full" :class="statusDotTone(flow.status, flow.error)" />
              <span class="mono text-[10.5px] font-semibold" :class="statusTone(flow.status, flow.error)">
                {{ statusCodeLabel(flow.status, flow.error) }}
              </span>
            </span>
            <span class="badge border-ink-700 bg-ink-950/30 text-mist-600">
              {{ protocolLabel(flow.protocol) }}
            </span>
            <span v-if="grpc" class="badge border-teal-500/25 bg-teal-500/[0.06] text-teal-300">gRPC</span>
            <span v-if="flow.status !== 101 && flow.tags?.length" class="badge border-ink-700 bg-ink-950/30 text-mist-600">
              {{ flow.tags[0] }}
            </span>
          </div>
          <div class="flex items-center gap-0.5">
            <button class="icon-btn icon-btn-sm" type="button" title="Refresh flow" aria-label="Refresh flow" @click="loadDetail(true)">
              <AppIcon name="refresh" :size="13" :class="loading ? 'animate-soft-spin' : ''" />
            </button>
            <button
              class="icon-btn icon-btn-sm text-rose-300/70 hover:text-rose-200"
              type="button"
              title="Delete this flow"
              aria-label="Delete this flow"
              @click="emit('delete')"
            >
              <AppIcon name="trash" :size="13" />
            </button>
            <button class="icon-btn icon-btn-sm" type="button" title="Close details" aria-label="Close details" @click="emit('close')">
              <AppIcon name="x" :size="14" />
            </button>
          </div>
        </div>

        <div class="px-3 pb-2 pt-1.5">
          <div class="flex items-start gap-1.5">
            <p class="url-line mono min-w-0 flex-1 text-[10.5px] leading-relaxed text-mist-300" :title="url">
              {{ url }}
            </p>
            <button class="icon-btn icon-btn-tiny mt-0.5 shrink-0" type="button" title="Copy URL" aria-label="Copy URL" @click="copyUrl">
              <AppIcon :name="urlCopied ? 'check' : 'copy'" :size="10" :class="urlCopied ? 'text-emerald-400' : ''" />
            </button>
          </div>
          <div class="mt-1.5 grid grid-cols-2 gap-x-3 gap-y-1 border-t border-ink-700/50 pt-1.5">
            <div class="flex min-w-0 items-center gap-1.5">
              <span class="w-[42px] shrink-0 text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">Client</span>
              <span class="mono truncate text-[9.5px] text-mist-500" :title="flow.client">{{ flow.client || '—' }}</span>
            </div>
            <div class="flex min-w-0 items-center gap-1.5">
              <span class="w-[42px] shrink-0 text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">Duration</span>
              <span class="mono truncate text-[9.5px] text-mist-400">{{ formatDuration(flow.duration_ms) }}</span>
            </div>
            <div class="flex min-w-0 items-center gap-1.5">
              <span class="w-[42px] shrink-0 text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">Request</span>
              <span class="mono truncate text-[9.5px] text-mist-500">{{ formatBytes(flow.request_size) }}</span>
            </div>
            <div class="flex min-w-0 items-center gap-1.5">
              <span class="w-[42px] shrink-0 text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">Response</span>
              <span class="mono truncate text-[9.5px] text-mist-500">{{ formatBytes(flow.response_size) }}</span>
            </div>
            <div class="col-span-2 flex min-w-0 items-center gap-1.5">
              <span class="w-[42px] shrink-0 text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">Started</span>
              <span class="mono truncate text-[9.5px] text-mist-500">{{ formatDateTime(flow.started_at) }}</span>
            </div>
            <div v-if="detail?.tls_sni" class="col-span-2 flex min-w-0 items-center gap-1.5">
              <span class="w-[42px] shrink-0 text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">TLS SNI</span>
              <span class="mono truncate text-[9.5px] text-mist-500">{{ detail.tls_sni }}</span>
            </div>
          </div>
        </div>

        <div
          v-if="hasError"
          class="mx-3 mb-2 flex items-start gap-1.5 rounded border border-rose-500/20 bg-rose-500/[0.045] px-2 py-1.5"
        >
          <AppIcon name="alert-triangle" :size="11" class="mt-0.5 shrink-0 text-rose-400" />
          <p class="break-words font-mono text-[9px] leading-relaxed text-rose-300/80">
            {{ detail?.error_detail || flow.error }}
          </p>
        </div>

        <nav class="app-scroll flex shrink-0 overflow-x-auto border-t border-ink-700/70" role="tablist" aria-label="Flow details">
          <button
            v-for="tab in tabItems"
            :key="tab.id"
            class="relative h-[34px] shrink-0 px-3 text-[10.5px] font-medium transition-colors"
            :class="activeTab === tab.id ? 'text-accent-300' : 'text-mist-600 hover:text-mist-400'"
            type="button"
            role="tab"
            :aria-selected="activeTab === tab.id"
            @click="selectTab(tab.id)"
          >
            {{ tab.label }}
            <span
              v-if="activeTab === tab.id"
              class="absolute inset-x-2 bottom-0 h-[2px] rounded-full bg-accent-400/80"
            />
          </button>
        </nav>
      </header>

      <div v-if="loading && !detail" class="min-h-0 flex-1 space-y-2 p-3">
        <div class="skeleton h-8 rounded" />
        <div class="skeleton h-4 w-2/3 rounded" />
        <div class="skeleton h-[46%] rounded-md" />
        <div class="skeleton h-[28%] rounded-md" />
      </div>

      <div v-else-if="error && !detail" class="flex min-h-0 flex-1 flex-col items-center justify-center px-6 text-center">
        <div class="flex h-11 w-11 items-center justify-center rounded-xl border border-rose-500/25 bg-rose-500/[0.05]">
          <AppIcon name="alert-triangle" :size="19" class="text-rose-400" />
        </div>
        <h3 class="mt-3 text-[12px] font-semibold text-mist-300">Could not load flow</h3>
        <p class="mt-1 max-w-[300px] break-words text-[10.5px] leading-relaxed text-mist-600">{{ error }}</p>
        <button class="btn btn-sm mt-3" type="button" @click="loadDetail()">
          <AppIcon name="refresh" :size="12" />
          Retry
        </button>
      </div>

      <div v-else-if="detail" class="min-h-0 flex-1 overflow-hidden">
        <BodyViewer
          v-if="activeTab === 'request'"
          :view="detail.request"
          direction="request"
          :filename-prefix="`mitmrs-flow-${flow.seq}`"
        />
        <BodyViewer
          v-else-if="activeTab === 'response'"
          :view="detail.response"
          direction="response"
          :filename-prefix="`mitmrs-flow-${flow.seq}`"
        />
        <div v-else-if="activeTab === 'headers'" class="app-scroll h-full overflow-auto">
          <section class="border-b border-ink-700/60">
            <div class="sticky top-0 z-[1] flex items-center gap-2 border-b border-ink-700/50 bg-ink-925/95 px-3 py-1.5 backdrop-blur">
              <span class="flex h-4 w-4 items-center justify-center rounded bg-sky-500/10 text-sky-400"><AppIcon name="arrow-up" :size="10" /></span>
              <span class="text-[10px] font-semibold text-mist-400">Request headers</span>
              <span class="font-mono text-[8.5px] text-mist-700">{{ requestHeaders.length }}</span>
              <button class="icon-btn icon-btn-tiny ml-auto" type="button" title="Copy request headers" @click="copyHeaders(detail.request, 'request')">
                <AppIcon :name="headersCopied === 'request' ? 'check' : 'copy'" :size="10" :class="headersCopied === 'request' ? 'text-emerald-400' : ''" />
              </button>
            </div>
            <div class="mono break-all px-3 py-1.5 text-[9.5px] text-mist-600">
              <span class="text-sky-400/80">{{ detail.method }}</span>
              {{ detail.path || '/' }}
              <span class="text-mist-700">{{ httpVersionLabel(detail.http_version) }}</span>
            </div>
            <div v-if="requestHeaders.length" class="px-3 pb-2">
              <div
                v-for="(header, index) in requestHeaders"
                :key="`${header.name}-${index}`"
                class="grid grid-cols-[minmax(92px,0.65fr)_minmax(0,1.8fr)] gap-2 border-t border-ink-700/30 py-1"
              >
                <span class="break-all font-mono text-[9.5px] text-sky-300/65">{{ header.name }}</span>
                <span class="break-all font-mono text-[9.5px] text-mist-400">{{ header.value }}</span>
              </div>
            </div>
            <p v-else class="px-3 pb-2 text-[10px] text-mist-700">No headers captured.</p>
            <div v-if="requestTrailers.length" class="px-3 pb-2">
              <p class="mb-1 text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">Trailers</p>
              <div v-for="(header, index) in requestTrailers" :key="`rt-${index}`" class="grid grid-cols-[minmax(92px,0.65fr)_minmax(0,1.8fr)] gap-2 border-t border-ink-700/30 py-1">
                <span class="break-all font-mono text-[9.5px] text-sky-300/65">{{ header.name }}</span>
                <span class="break-all font-mono text-[9.5px] text-mist-400">{{ header.value }}</span>
              </div>
            </div>
          </section>

          <section>
            <div class="sticky top-0 z-[1] flex items-center gap-2 border-b border-ink-700/50 bg-ink-925/95 px-3 py-1.5 backdrop-blur">
              <span class="flex h-4 w-4 items-center justify-center rounded bg-teal-500/10 text-teal-400"><AppIcon name="arrow-down" :size="10" /></span>
              <span class="text-[10px] font-semibold text-mist-400">Response headers</span>
              <span class="font-mono text-[8.5px] text-mist-700">{{ responseHeaders.length }}</span>
              <button class="icon-btn icon-btn-tiny ml-auto" type="button" title="Copy response headers" @click="copyHeaders(detail.response, 'response')">
                <AppIcon :name="headersCopied === 'response' ? 'check' : 'copy'" :size="10" :class="headersCopied === 'response' ? 'text-emerald-400' : ''" />
              </button>
            </div>
            <div class="mono px-3 py-1.5 text-[9.5px] text-mist-600">
              <span class="text-teal-400/80">{{ httpVersionLabel(detail.http_version) }}</span>
              <span :class="statusTone(detail.status, detail.error)"> {{ statusCodeLabel(detail.status, detail.error) }}</span>
            </div>
            <div v-if="responseHeaders.length" class="px-3 pb-2">
              <div
                v-for="(header, index) in responseHeaders"
                :key="`${header.name}-${index}`"
                class="grid grid-cols-[minmax(92px,0.65fr)_minmax(0,1.8fr)] gap-2 border-t border-ink-700/30 py-1"
              >
                <span class="break-all font-mono text-[9.5px] text-teal-300/65">{{ header.name }}</span>
                <span class="break-all font-mono text-[9.5px] text-mist-400">{{ header.value }}</span>
              </div>
            </div>
            <p v-else class="px-3 pb-2 text-[10px] text-mist-700">No headers captured.</p>
            <div v-if="responseTrailers.length" class="px-3 pb-2">
              <p class="mb-1 text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">Trailers</p>
              <div v-for="(header, index) in responseTrailers" :key="`rst-${index}`" class="grid grid-cols-[minmax(92px,0.65fr)_minmax(0,1.8fr)] gap-2 border-t border-ink-700/30 py-1">
                <span class="break-all font-mono text-[9.5px] text-teal-300/65">{{ header.name }}</span>
                <span class="break-all font-mono text-[9.5px] text-mist-400">{{ header.value }}</span>
              </div>
            </div>
          </section>
        </div>
        <DecodePanel v-else-if="activeTab === 'decode'" :flow="detail" />
        <CurlPanel v-else-if="activeTab === 'curl'" :flow="detail" />
        <McpTab v-else-if="activeTab === 'mcp'" :flow="detail" :endpoint="mcpEndpoint" />
      </div>
    </template>
  </section>
</template>

<style scoped>
.url-line {
  display: -webkit-box;
  overflow: hidden;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow-wrap: anywhere;
}

@media (min-width: 1280px) {
  .url-line {
    -webkit-line-clamp: 3;
  }
}
</style>
