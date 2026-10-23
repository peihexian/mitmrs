<script setup lang="ts">
import {
  computed,
  nextTick,
  onBeforeUnmount,
  onMounted,
  ref,
  watch,
} from 'vue'
import AppIcon from './AppIcon.vue'
import type { FlowSummary, ProxyState } from '../types'
import {
  formatBytes,
  formatClock,
  formatDateTime,
  formatDuration,
  formatNumber,
  flowUrl,
  isGrpc,
  methodTone,
  protocolLabel,
  statusCodeLabel,
  statusDotTone,
  statusTone,
} from '../lib/format'

const props = withDefaults(
  defineProps<{
    flows: FlowSummary[]
    selectedId: string | null
    total: number
    loading: boolean
    hasFilters: boolean
    proxyAddress: string
    mcpEndpoint: string
    proxyState: ProxyState | 'connecting'
    page: number
    pageSize: number
    pageCount: number
    error?: string | null
  }>(),
  { error: null },
)

const emit = defineEmits<{
  select: [id: string]
  clear: []
  refresh: []
  start: []
  page: [page: number]
  'page-size': [size: number]
  'copy-address': []
  'open-mcp': []
  'open-ca': []
}>()

const ROW_HEIGHT = 38
const OVERSCAN = 12
const viewport = ref<HTMLDivElement | null>(null)
const scrollTop = ref(0)
const viewportHeight = ref(600)
let resizeObserver: ResizeObserver | null = null

const visibleStart = computed(() =>
  Math.max(0, Math.floor(scrollTop.value / ROW_HEIGHT) - OVERSCAN),
)
const visibleEnd = computed(() =>
  Math.min(
    props.flows.length,
    Math.ceil((scrollTop.value + viewportHeight.value) / ROW_HEIGHT) + OVERSCAN,
  ),
)
const visibleRows = computed(() =>
  props.flows.slice(visibleStart.value, visibleEnd.value).map((flow, index) => ({
    flow,
    index: visibleStart.value + index,
  })),
)
const totalHeight = computed(() => props.flows.length * ROW_HEIGHT)
const showSetup = computed(
  () => !props.flows.length && !props.loading && !props.hasFilters && !props.error,
)
const showFilterEmpty = computed(
  () => !props.flows.length && !props.loading && props.hasFilters && !props.error,
)
const selectedIndex = computed(() =>
  props.flows.findIndex((flow) => flow.id === props.selectedId),
)
const pageStart = computed(() =>
  props.total === 0 ? 0 : (props.page - 1) * props.pageSize + 1,
)
const pageEnd = computed(() => pageStart.value + Math.max(0, props.flows.length - 1))

function selectFlow(id: string): void {
  emit('select', id)
  viewport.value?.focus({ preventScroll: true })
}

function ensureSelectedVisible(): void {
  const index = selectedIndex.value
  if (index < 0 || !viewport.value) return
  const top = index * ROW_HEIGHT
  const bottom = top + ROW_HEIGHT
  const currentTop = viewport.value.scrollTop
  const currentBottom = currentTop + viewport.value.clientHeight
  if (top < currentTop) {
    viewport.value.scrollTop = top
  } else if (bottom > currentBottom) {
    viewport.value.scrollTop = bottom - viewport.value.clientHeight
  }
}

function selectIndex(index: number): void {
  if (!props.flows.length) return
  const clamped = Math.max(0, Math.min(props.flows.length - 1, index))
  const flow = props.flows[clamped]
  if (flow) emit('select', flow.id)
}

function onKeydown(event: KeyboardEvent): void {
  const current = selectedIndex.value
  const page = Math.max(1, Math.floor(viewportHeight.value / ROW_HEIGHT) - 2)
  switch (event.key) {
    case 'ArrowDown':
      event.preventDefault()
      selectIndex(current < 0 ? 0 : current + 1)
      break
    case 'ArrowUp':
      event.preventDefault()
      selectIndex(current < 0 ? 0 : current - 1)
      break
    case 'Home':
      event.preventDefault()
      selectIndex(0)
      break
    case 'End':
      event.preventDefault()
      selectIndex(props.flows.length - 1)
      break
    case 'PageDown':
      event.preventDefault()
      selectIndex(current < 0 ? 0 : current + page)
      break
    case 'PageUp':
      event.preventDefault()
      selectIndex(current < 0 ? 0 : current - page)
      break
    case 'Escape':
      viewport.value?.blur()
      break
    default:
      break
  }
}

function onScroll(): void {
  scrollTop.value = viewport.value?.scrollTop ?? 0
}

function onPageSizeChange(event: Event): void {
  const value = Number((event.target as HTMLSelectElement).value)
  if (Number.isFinite(value) && value > 0) emit('page-size', value)
}

function measure(): void {
  if (!viewport.value) return
  viewportHeight.value = viewport.value.clientHeight
  scrollTop.value = viewport.value.scrollTop
}

watch([() => props.selectedId, () => props.flows], async () => {
  await nextTick()
  ensureSelectedVisible()
})

watch(
  () => props.page,
  async () => {
    await nextTick()
    if (viewport.value) {
      viewport.value.scrollTop = 0
      scrollTop.value = 0
    }
  },
)

watch(
  viewport,
  (element) => {
    resizeObserver?.disconnect()
    resizeObserver = null
    if (!element) return
    measure()
    resizeObserver = new ResizeObserver(measure)
    resizeObserver.observe(element)
  },
  { flush: 'post' },
)

onMounted(measure)

onBeforeUnmount(() => {
  resizeObserver?.disconnect()
  resizeObserver = null
})
</script>

<template>
  <div class="flow-table relative flex min-h-0 flex-1 flex-col overflow-hidden">
    <div class="flow-head flow-grid shrink-0" role="row">
      <div class="col-seq flex items-center gap-1 text-[9px] font-semibold uppercase tracking-wider text-mist-600">
        <span>#</span>
        <AppIcon name="arrow-down" :size="9" class="text-mist-700" />
      </div>
      <div class="col-time text-[9px] font-semibold uppercase tracking-wider text-mist-600">
        Time
      </div>
      <div class="text-[9px] font-semibold uppercase tracking-wider text-mist-600">Method</div>
      <div class="text-[9px] font-semibold uppercase tracking-wider text-mist-600">
        Host / Path
      </div>
      <div class="text-[9px] font-semibold uppercase tracking-wider text-mist-600">Status</div>
      <div class="col-size text-right text-[9px] font-semibold uppercase tracking-wider text-mist-600">
        Size
      </div>
      <div class="col-duration text-right text-[9px] font-semibold uppercase tracking-wider text-mist-600">
        Dur
      </div>
      <div class="col-protocol text-[9px] font-semibold uppercase tracking-wider text-mist-600">
        Proto
      </div>
    </div>

    <div v-if="loading && !flows.length" class="min-h-0 flex-1 overflow-hidden">
      <div
        v-for="index in 11"
        :key="index"
        class="flow-grid flow-skeleton-row"
        aria-hidden="true"
      >
        <div class="skeleton h-2 w-5 rounded" />
        <div class="col-time skeleton h-2 w-10 rounded" />
        <div class="skeleton h-2 w-8 rounded" />
        <div class="skeleton h-2 rounded" :style="{ width: `${38 + ((index * 17) % 45)}%` }" />
        <div class="skeleton h-2 w-7 rounded" />
        <div class="col-size skeleton ml-auto h-2 w-8 rounded" />
        <div class="col-duration skeleton ml-auto h-2 w-8 rounded" />
        <div class="col-protocol skeleton h-2 w-6 rounded" />
      </div>
    </div>

    <div
      v-else-if="error && !flows.length"
      class="flex min-h-0 flex-1 flex-col items-center justify-center px-6 text-center"
    >
      <div class="flex h-11 w-11 items-center justify-center rounded-xl border border-rose-500/25 bg-rose-500/[0.06]">
        <AppIcon name="alert-triangle" :size="21" class="text-rose-400" />
      </div>
      <h2 class="mt-3 text-sm font-semibold text-mist-200">Traffic stream unavailable</h2>
      <p class="mt-1 max-w-sm text-[11.5px] leading-relaxed text-mist-500">{{ error }}</p>
      <button class="btn btn-sm mt-4" type="button" @click="emit('refresh')">
        <AppIcon name="refresh" :size="13" />
        Try again
      </button>
    </div>

    <div
      v-else-if="showSetup"
      class="app-scroll min-h-0 flex-1"
    >
      <div class="mx-auto flex min-h-full w-full max-w-[680px] flex-col justify-center px-5 py-8">
        <div class="empty-hero relative overflow-hidden rounded-xl border border-ink-700/80 bg-ink-900/55 p-5 shadow-[0_18px_60px_var(--ui-shadow-hero)] sm:p-6">
          <div class="pointer-events-none absolute -right-20 -top-24 h-52 w-52 rounded-full bg-accent-500/[0.055] blur-2xl" />
          <div class="pointer-events-none absolute -bottom-28 -left-20 h-52 w-52 rounded-full bg-teal-500/[0.04] blur-2xl" />
          <div class="relative flex items-start gap-4">
            <div class="relative flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-accent-500/25 bg-ink-950/70">
              <span class="absolute inset-1 rounded-lg border border-accent-500/10" />
              <AppIcon name="radio" :size="21" class="text-accent-400" />
            </div>
            <div class="min-w-0">
              <div class="flex flex-wrap items-center gap-2">
                <h2 class="text-[15px] font-semibold tracking-tight text-mist-200">Waiting for traffic</h2>
                <span
                  class="rounded border px-1.5 py-px text-[8.5px] font-semibold uppercase tracking-[0.08em]"
                  :class="
                    proxyState === 'running'
                      ? 'border-emerald-500/25 bg-emerald-500/[0.06] text-emerald-300'
                      : 'border-ink-700 bg-ink-800/50 text-mist-500'
                  "
                >{{ proxyState === 'running' ? 'Listener ready' : 'Listener idle' }}</span>
              </div>
              <p class="mt-1 max-w-md text-[11.5px] leading-relaxed text-mist-500">
                Point a browser, app, or AI agent at the local proxy. Requests
                and responses will appear here in real time.
              </p>
            </div>
          </div>

          <div class="relative mt-5">
            <div class="field-label">Proxy address</div>
            <div class="flex items-stretch gap-1.5">
              <div class="mono flex min-w-0 flex-1 items-center gap-2 rounded-md border border-ink-700 bg-ink-950/60 px-2.5 py-2 text-[11.5px] text-accent-300/90">
                <AppIcon name="plug" :size="13" class="text-mist-600" />
                <span class="truncate">{{ proxyAddress }}</span>
              </div>
              <button class="btn" type="button" @click="emit('copy-address')">
                <AppIcon name="copy" :size="13" />
                <span class="hidden sm:inline">Copy</span>
              </button>
              <button
                v-if="proxyState !== 'running' && proxyState !== 'starting'"
                class="btn btn-primary"
                type="button"
                @click="emit('start')"
              >
                <AppIcon name="play" :size="13" />
                Start
              </button>
            </div>
            <div class="mono mt-2 flex items-center gap-2 overflow-hidden rounded border border-ink-700/60 bg-ink-950/40 px-2.5 py-1.5 text-[9.5px] text-mist-600">
              <span class="text-accent-500/60">$</span>
              <span class="truncate">curl -x http://{{ proxyAddress }} https://httpbin.org/json</span>
            </div>
          </div>

          <div class="relative mt-5 grid gap-2 sm:grid-cols-3">
            <button
              class="group flex items-start gap-2.5 rounded-lg border border-ink-700/80 bg-ink-950/25 p-3 text-left transition hover:border-ink-600 hover:bg-ink-850/50"
              type="button"
              @click="emit('open-ca')"
            >
              <span class="flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-ink-700 bg-ink-900 text-mist-500 group-hover:text-accent-300">
                <AppIcon name="shield-check" :size="13" />
              </span>
              <span class="min-w-0">
                <span class="block text-[10.5px] font-semibold text-mist-300">Trust the CA</span>
                <span class="mt-0.5 block text-[9.5px] leading-snug text-mist-600">Enable HTTPS inspection</span>
              </span>
            </button>
            <button
              class="group flex items-start gap-2.5 rounded-lg border border-ink-700/80 bg-ink-950/25 p-3 text-left transition hover:border-ink-600 hover:bg-ink-850/50"
              type="button"
              @click="emit('open-mcp')"
            >
              <span class="flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-ink-700 bg-ink-900 text-mist-500 group-hover:text-accent-300">
                <AppIcon name="sparkles" :size="13" />
              </span>
              <span class="min-w-0">
                <span class="block text-[10.5px] font-semibold text-mist-300">Connect an AI agent</span>
                <span class="mt-0.5 block truncate font-mono text-[9px] text-mist-600" :title="mcpEndpoint">{{ mcpEndpoint }}</span>
              </span>
            </button>
            <button
              class="group flex items-start gap-2.5 rounded-lg border border-ink-700/80 bg-ink-950/25 p-3 text-left transition hover:border-ink-600 hover:bg-ink-850/50"
              type="button"
              @click="emit('refresh')"
            >
              <span class="flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-ink-700 bg-ink-900 text-mist-500 group-hover:text-accent-300">
                <AppIcon name="refresh" :size="13" />
              </span>
              <span class="min-w-0">
                <span class="block text-[10.5px] font-semibold text-mist-300">Refresh now</span>
                <span class="mt-0.5 block text-[9.5px] leading-snug text-mist-600">Reconnect the stream</span>
              </span>
            </button>
          </div>
        </div>

        <div class="mt-3 flex flex-wrap items-center justify-center gap-x-3 gap-y-1 text-[9.5px] text-mist-700">
          <span class="flex items-center gap-1"><AppIcon name="lock" :size="10" /> Local capture</span>
          <span class="h-1 w-1 rounded-full bg-ink-650" />
          <span class="flex items-center gap-1"><span class="kbd">↑</span><span class="kbd">↓</span> navigate</span>
          <span class="h-1 w-1 rounded-full bg-ink-650" />
          <span class="flex items-center gap-1"><span class="kbd">/</span> search</span>
        </div>
      </div>
    </div>

    <div
      v-else-if="showFilterEmpty"
      class="flex min-h-0 flex-1 flex-col items-center justify-center px-6 text-center"
    >
      <div class="flex h-11 w-11 items-center justify-center rounded-xl border border-ink-700 bg-ink-900/50">
        <AppIcon name="search" :size="20" class="text-mist-600" />
      </div>
      <h2 class="mt-3 text-sm font-semibold text-mist-300">No matching traffic</h2>
      <p class="mt-1 max-w-xs text-[11.5px] leading-relaxed text-mist-600">
        No captured flows match the current filters. Adjust the search or clear
        the filters to see all traffic.
      </p>
      <button class="btn btn-sm mt-4" type="button" @click="emit('clear')">
        <AppIcon name="x" :size="12" />
        Clear filters
      </button>
    </div>

    <div
      v-else
      ref="viewport"
      class="flow-scroll relative min-h-0 flex-1 overflow-auto outline-none"
      tabindex="0"
      role="rowgroup"
      aria-label="Captured HTTP flows"
      @scroll.passive="onScroll"
      @keydown="onKeydown"
    >
      <div class="relative w-full" :style="{ height: `${totalHeight}px` }">
        <div
          v-for="row in visibleRows"
          :key="row.flow.id"
          class="flow-row flow-grid absolute inset-x-0"
          :class="{
            'is-selected': row.flow.id === selectedId,
            'is-error': Boolean(row.flow.error),
          }"
          :style="{ transform: `translateY(${row.index * ROW_HEIGHT}px)`, height: `${ROW_HEIGHT}px` }"
          role="row"
          :aria-selected="row.flow.id === selectedId"
          :title="flowUrl(row.flow)"
          @click="selectFlow(row.flow.id)"
        >
          <div class="col-seq mono text-[10px] tabular-nums text-mist-650">
            {{ row.flow.seq }}
          </div>
          <div
            class="col-time mono text-[10px] tabular-nums text-mist-600"
            :title="formatDateTime(row.flow.started_at)"
          >
            {{ formatClock(row.flow.started_at) }}
          </div>
          <div class="mono text-[10.5px] font-semibold tracking-tight" :class="methodTone(row.flow.method)">
            {{ row.flow.method }}
          </div>
          <div class="flex min-w-0 items-center gap-1.5">
            <span class="min-w-0 max-w-[52%] truncate text-[11px] text-mist-300">
              {{ row.flow.host }}<span
                v-if="row.flow.port && row.flow.port !== 443 && row.flow.port !== 80"
                class="text-mist-650"
              >:{{ row.flow.port }}</span>
            </span>
            <span class="min-w-0 flex-1 truncate font-mono text-[10.5px] text-mist-500">
              {{ row.flow.path || '/' }}
            </span>
            <span
              v-if="row.flow.error"
              class="flex h-4 w-4 shrink-0 items-center justify-center rounded bg-rose-500/10 text-rose-400"
              :title="row.flow.error"
            >
              <AppIcon name="alert-triangle" :size="10" />
            </span>
          </div>
          <div class="flex items-center gap-1.5">
            <span class="h-1.5 w-1.5 shrink-0 rounded-full" :class="statusDotTone(row.flow.status, row.flow.error)" />
            <span class="mono text-[10.5px] font-medium tabular-nums" :class="statusTone(row.flow.status, row.flow.error)">
              {{ statusCodeLabel(row.flow.status, row.flow.error) }}
            </span>
          </div>
          <div
            class="col-size mono text-right text-[10px] tabular-nums text-mist-500"
            :title="`Request ${formatBytes(row.flow.request_size)} · Response ${formatBytes(row.flow.response_size)}`"
          >
            {{ formatBytes(row.flow.response_size || row.flow.request_size) }}
          </div>
          <div
            class="col-duration mono text-right text-[10px] tabular-nums"
            :class="
              row.flow.duration_ms != null && row.flow.duration_ms > 1000
                ? 'text-amber-500/80'
                : 'text-mist-600'
            "
          >
            {{ formatDuration(row.flow.duration_ms) }}
          </div>
          <div class="col-protocol min-w-0">
            <span
              v-if="isGrpc(row.flow)"
              class="badge border-teal-500/25 bg-teal-500/[0.07] text-teal-300"
              title="gRPC"
            >gRPC</span>
            <span v-else class="mono text-[9.5px] text-mist-600" :title="row.flow.protocol">
              {{ protocolLabel(row.flow.protocol) }}
            </span>
          </div>
        </div>
      </div>
    </div>

    <div
      v-if="loading && flows.length"
      class="pointer-events-none absolute inset-x-0 top-[33px] z-10 h-px overflow-hidden bg-accent-500/10"
    >
      <div class="h-full w-1/3 animate-pulse bg-accent-400/70" />
    </div>

    <div
      v-if="total > 0"
      class="flex h-8 shrink-0 items-center justify-between gap-2 border-t border-ink-700/60 bg-ink-925/60 px-3 text-[9px] text-mist-700"
    >
      <span class="mono tabular-nums">
        Rows {{ formatNumber(pageStart) }}–{{ formatNumber(pageEnd) }} of
        {{ formatNumber(total) }}
      </span>
      <div class="flex items-center gap-1">
        <label class="hidden items-center gap-1 sm:flex">
          <span>Per page</span>
          <select
            class="h-5 rounded border border-ink-700 bg-ink-950 px-1 text-[9px] text-mist-400 outline-none"
            :value="pageSize"
            aria-label="Rows per page"
            @change="onPageSizeChange"
          >
            <option :value="100">100</option>
            <option :value="200">200</option>
            <option :value="500">500</option>
          </select>
        </label>
        <button
          class="icon-btn icon-btn-tiny"
          type="button"
          title="Previous page"
          aria-label="Previous page"
          :disabled="page <= 1"
          @click="emit('page', page - 1)"
        >
          <AppIcon name="chevron-right" :size="10" class="rotate-180" />
        </button>
        <span class="mono min-w-[62px] text-center tabular-nums">
          Page {{ formatNumber(page) }} / {{ formatNumber(Math.max(1, pageCount)) }}
        </span>
        <button
          class="icon-btn icon-btn-tiny"
          type="button"
          title="Next page"
          aria-label="Next page"
          :disabled="page >= pageCount"
          @click="emit('page', page + 1)"
        >
          <AppIcon name="chevron-right" :size="10" />
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.flow-head {
  height: 33px;
  border-bottom: 1px solid var(--ui-hairline);
  background: var(--ui-table-head);
  padding: 0 8px;
}

.flow-grid {
  display: grid;
  align-items: center;
  column-gap: 8px;
  padding-left: 8px;
  padding-right: 8px;
  grid-template-columns: 42px 52px minmax(0, 1fr) 48px;
}

.col-time,
.col-size,
.col-duration,
.col-protocol {
  display: none;
}

.flow-row {
  top: 0;
  cursor: pointer;
  border-bottom: 1px solid var(--ui-hairline-soft);
  background: transparent;
  transition:
    background-color 90ms ease,
    box-shadow 90ms ease;
  outline: none;
}

.flow-row:hover {
  background: var(--ui-row-hover);
}

.flow-row.is-selected {
  background: var(--ui-row-selected);
  box-shadow: inset 2px 0 0 var(--ui-row-selected-line);
}

.flow-row.is-error:not(.is-selected) {
  background: var(--ui-row-error);
  box-shadow: inset 2px 0 0 var(--ui-row-error-line);
}

.flow-row:focus-visible {
  box-shadow:
    inset 2px 0 0 var(--ui-row-selected-line-strong),
    inset 0 0 0 1px var(--ui-row-selected-ring);
}

.flow-skeleton-row {
  height: 38px;
  border-bottom: 1px solid var(--ui-hairline-faint);
}

.flow-table {
  container-type: inline-size;
}

.flow-scroll {
  scrollbar-gutter: stable;
}

.empty-hero {
  background-image: var(--ui-hero-bg);
}

@media (min-width: 640px) {
  .flow-grid {
    grid-template-columns: 44px 52px minmax(0, 1fr) 50px 62px;
  }

  .col-size {
    display: block;
  }
}

@media (min-width: 768px) {
  .flow-grid {
    grid-template-columns: 44px 70px 52px minmax(0, 1fr) 52px 64px 56px;
  }

  .col-time,
  .col-protocol {
    display: block;
  }
}

@media (min-width: 1024px) {
  .flow-grid {
    grid-template-columns: 48px 76px 54px minmax(0, 1fr) 54px 66px 60px 56px;
  }

  .col-duration {
    display: block;
  }

  /* When the side detail panel squeezes the table, reclaim space from
     secondary columns while keeping the host/path readable. */
  @container (max-width: 660px) {
    .flow-grid {
      grid-template-columns: 48px 54px minmax(0, 1fr) 54px 60px 56px;
    }

    .col-time,
    .col-size {
      display: none;
    }
  }
}
</style>
