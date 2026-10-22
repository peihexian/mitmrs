<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import AppIcon from './AppIcon.vue'
import type { FlowFilters, Stats, Status } from '../types'
import {
  activeFilterCount,
  formatBytes,
  formatNumber,
} from '../lib/format'

const props = withDefaults(
  defineProps<{
    stats: Stats | null
    status: Status | null
    totalFlows: number
    showClose?: boolean
  }>(),
  { showClose: false },
)

const filters = defineModel<FlowFilters>('filters', { required: true })

const emit = defineEmits<{
  clear: []
  close: []
  resume: []
  'open-settings': []
}>()

const searchInput = ref<HTMLInputElement | null>(null)
const methods = [
  { value: '', label: 'Any method' },
  { value: 'GET', label: 'GET' },
  { value: 'POST', label: 'POST' },
  { value: 'PUT', label: 'PUT' },
  { value: 'PATCH', label: 'PATCH' },
  { value: 'DELETE', label: 'DELETE' },
  { value: 'HEAD', label: 'HEAD' },
  { value: 'OPTIONS', label: 'OPTIONS' },
  { value: 'CONNECT', label: 'CONNECT' },
]
const protocols = [
  { value: '', label: 'Any protocol' },
  { value: 'http/1.1', label: 'HTTP/1.1' },
  { value: 'http/2', label: 'HTTP/2' },
  { value: 'grpc', label: 'gRPC' },
  { value: 'sse', label: 'SSE' },
  { value: 'ndjson', label: 'NDJSON' },
  { value: 'websocket', label: 'WebSocket' },
  { value: 'tls', label: 'TLS' },
  { value: 'other', label: 'Other' },
]
const statusClasses = ['2xx', '3xx', '4xx', '5xx', 'error']

const count = computed(() => activeFilterCount(filters.value))
const hostOptions = computed(() => {
  const hosts = (props.stats?.hosts ?? []).map((item) => item.host)
  if (filters.value.host && !hosts.includes(filters.value.host)) {
    hosts.unshift(filters.value.host)
  }
  return hosts
})
const topHosts = computed(() => (props.stats?.hosts ?? []).slice(0, 7))
const maxHostCount = computed(() =>
  Math.max(1, ...(props.stats?.hosts ?? []).map((item) => item.count)),
)
const isPaused = computed(
  () =>
    props.status?.proxy.state === 'paused' ||
    props.status?.config?.capture_enabled === false,
)
const pausedLabel = computed(() =>
  props.status?.config?.capture_enabled === false
    ? 'Capture disabled in settings'
    : 'Capture paused',
)

function setHost(host: string): void {
  filters.value.host = filters.value.host === host ? '' : host
}

function clearSearch(): void {
  filters.value.q = ''
  searchInput.value?.focus()
}

function onGlobalKeydown(event: KeyboardEvent): void {
  const target = event.target as HTMLElement | null
  const editable =
    target?.tagName === 'INPUT' ||
    target?.tagName === 'TEXTAREA' ||
    target?.tagName === 'SELECT' ||
    target?.isContentEditable
  if (
    event.key === '/' &&
    !editable &&
    !event.ctrlKey &&
    !event.metaKey &&
    !event.altKey &&
    searchInput.value &&
    searchInput.value.offsetParent !== null
  ) {
    event.preventDefault()
    searchInput.value.focus()
  }
  if (
    event.key === 'Escape' &&
    document.activeElement === searchInput.value &&
    searchInput.value?.value
  ) {
    event.preventDefault()
    clearSearch()
  }
}

onMounted(() => window.addEventListener('keydown', onGlobalKeydown))
onBeforeUnmount(() => window.removeEventListener('keydown', onGlobalKeydown))
</script>

<template>
  <aside class="flex h-full min-h-0 w-full flex-col bg-ink-925/75">
    <div class="flex h-11 shrink-0 items-center justify-between border-b border-ink-700/70 px-3">
      <div class="flex items-center gap-2">
        <AppIcon name="filter" :size="13" class="text-mist-600" />
        <span class="text-[11px] font-semibold uppercase tracking-[0.09em] text-mist-400">
          Filters
        </span>
        <span
          v-if="count > 0"
          class="rounded-full border border-accent-500/25 bg-accent-500/10 px-1.5 py-px text-[9px] font-semibold text-accent-300"
        >{{ count }}</span>
      </div>
      <div class="flex items-center gap-0.5">
        <button
          v-if="count > 0"
          class="btn btn-sm btn-ghost h-6 px-1.5 text-[10.5px]"
          type="button"
          @click="emit('clear')"
        >
          Reset
        </button>
        <button
          v-if="showClose"
          class="icon-btn icon-btn-sm"
          type="button"
          aria-label="Close filters"
          @click="emit('close')"
        >
          <AppIcon name="x" :size="15" />
        </button>
      </div>
    </div>

    <div class="app-scroll min-h-0 flex-1 px-3 pb-4 pt-3">
      <div class="relative">
        <AppIcon
          name="search"
          :size="14"
          class="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-mist-600"
        />
        <input
          ref="searchInput"
          v-model="filters.q"
          type="search"
          class="field pl-8 pr-12"
          placeholder="Search traffic…"
          autocomplete="off"
          spellcheck="false"
          aria-label="Search traffic"
        />
        <div class="absolute right-1.5 top-1/2 flex -translate-y-1/2 items-center">
          <button
            v-if="filters.q"
            class="icon-btn icon-btn-tiny"
            type="button"
            aria-label="Clear search"
            @click="clearSearch"
          >
            <AppIcon name="x" :size="11" />
          </button>
          <span v-else class="kbd">/</span>
        </div>
      </div>

      <div
        v-if="isPaused"
        class="mt-2.5 flex items-start gap-2 rounded-md border border-amber-500/20 bg-amber-500/[0.06] px-2.5 py-2"
      >
        <AppIcon name="pause" :size="13" class="mt-0.5 shrink-0 text-amber-400" />
        <div class="min-w-0 flex-1">
          <p class="text-[10.5px] font-semibold text-amber-200">{{ pausedLabel }}</p>
          <p class="mt-0.5 text-[10px] leading-snug text-amber-200/50">
            Existing captures remain available.
          </p>
          <button
            v-if="status?.proxy.state === 'paused'"
            class="mt-1 text-[10px] font-semibold text-amber-300 underline decoration-amber-500/40 underline-offset-2 hover:text-amber-200"
            type="button"
            @click="emit('resume')"
          >
            Resume capture
          </button>
          <button
            v-else
            class="mt-1 text-[10px] font-semibold text-amber-300 underline decoration-amber-500/40 underline-offset-2 hover:text-amber-200"
            type="button"
            @click="emit('open-settings')"
          >
            Open settings
          </button>
        </div>
      </div>

      <div class="mt-4 space-y-3.5">
        <div>
          <label class="field-label" for="filter-method">
            <span>Method</span>
            <span v-if="filters.method" class="normal-case tracking-normal text-mist-700">1</span>
          </label>
          <select id="filter-method" v-model="filters.method" class="field">
            <option v-for="method in methods" :key="method.value" :value="method.value">
              {{ method.label }}
            </option>
          </select>
        </div>

        <div>
          <div class="field-label">
            <span>Status class</span>
            <button
              v-if="filters.status"
              class="normal-case tracking-normal text-mist-600 hover:text-mist-400"
              type="button"
              @click="filters.status = ''"
            >clear</button>
          </div>
          <div class="grid grid-cols-5 gap-1">
            <button
              v-for="statusClass in statusClasses"
              :key="statusClass"
              class="flex h-[27px] items-center justify-center rounded border font-mono text-[9.5px] font-semibold transition"
              :class="
                filters.status === statusClass
                  ? statusClass === 'error'
                    ? 'border-rose-500/50 bg-rose-500/10 text-rose-300'
                    : 'border-accent-500/40 bg-accent-500/10 text-accent-300'
                  : 'border-ink-700 bg-ink-950/30 text-mist-600 hover:border-ink-600 hover:text-mist-400'
              "
              type="button"
              :aria-pressed="filters.status === statusClass"
              @click="filters.status = filters.status === statusClass ? '' : statusClass"
            >
              {{ statusClass }}
            </button>
          </div>
        </div>

        <div>
          <label class="field-label" for="filter-protocol">Protocol</label>
          <select id="filter-protocol" v-model="filters.protocol" class="field">
            <option v-for="protocol in protocols" :key="protocol.value" :value="protocol.value">
              {{ protocol.label }}
            </option>
          </select>
        </div>

        <div>
          <label class="field-label" for="filter-host">Host</label>
          <select id="filter-host" v-model="filters.host" class="field">
            <option value="">All hosts</option>
            <option v-for="host in hostOptions" :key="host" :value="host">
              {{ host }}
            </option>
          </select>
        </div>

        <div>
          <label class="field-label" for="filter-mime">MIME contains</label>
          <input
            id="filter-mime"
            v-model="filters.mime"
            class="field field-mono"
            placeholder="e.g. json, image/"
            autocomplete="off"
            spellcheck="false"
          />
        </div>

        <div>
          <div class="field-label">
            <span>Direction</span>
          </div>
          <div class="segmented w-full">
            <button
              v-for="direction in [
                { value: 'both', label: 'Both' },
                { value: 'request', label: 'Request' },
                { value: 'response', label: 'Response' },
              ]"
              :key="direction.value"
              class="flex-1 justify-center"
              :class="{ 'is-active': filters.direction === direction.value }"
              type="button"
              @click="filters.direction = direction.value as 'both' | 'request' | 'response'"
            >
              {{ direction.label }}
            </button>
          </div>
        </div>

        <div>
          <label class="field-label" for="filter-body">
            <span>Body contains</span>
            <span class="normal-case tracking-normal text-mist-700">optional</span>
          </label>
          <input
            id="filter-body"
            v-model="filters.body_contains"
            class="field field-mono"
            placeholder="substring…"
            autocomplete="off"
            spellcheck="false"
          />
        </div>
      </div>

      <div class="my-4 h-px bg-ink-700/60" />

      <section>
        <div class="flex items-center justify-between">
          <h3 class="section-title">Top hosts</h3>
          <span class="mono text-[9px] text-mist-700">{{ stats?.hosts.length ?? 0 }}</span>
        </div>
        <div v-if="topHosts.length" class="mt-2 space-y-0.5">
          <button
            v-for="item in topHosts"
            :key="item.host"
            class="group relative flex w-full items-center gap-2 overflow-hidden rounded px-1.5 py-[5px] text-left transition hover:bg-white/[0.035]"
            :class="filters.host === item.host ? 'bg-accent-500/[0.055]' : ''"
            type="button"
            :title="`Filter by ${item.host}`"
            @click="setHost(item.host)"
          >
            <span
              class="absolute inset-y-0 left-0 -z-0 bg-accent-500/[0.055] transition-all group-hover:bg-accent-500/[0.08]"
              :style="{ width: `${Math.max(2, (item.count / maxHostCount) * 100)}%` }"
              :class="filters.host === item.host ? 'bg-accent-500/[0.1]' : ''"
            />
            <span
              class="relative z-10 min-w-0 flex-1 truncate font-mono text-[10.5px]"
              :class="filters.host === item.host ? 'text-accent-300' : 'text-mist-400 group-hover:text-mist-300'"
            >{{ item.host }}</span>
            <span class="relative z-10 font-mono text-[9.5px] tabular-nums text-mist-600">
              {{ formatNumber(item.count) }}
            </span>
          </button>
        </div>
        <p v-else class="mt-2 text-[10.5px] leading-relaxed text-mist-700">
          Hosts appear here as traffic is captured.
        </p>
      </section>

      <div v-if="status" class="mt-4 rounded-md border border-ink-700/60 bg-ink-950/30 px-2.5 py-2">
        <div class="flex items-center gap-1.5">
          <AppIcon
            :name="status.proxy.windivert.enabled ? 'shield-check' : 'info'"
            :size="11"
            :class="status.proxy.windivert.enabled ? 'text-teal-500' : 'text-mist-600'"
          />
          <span class="text-[10px] text-mist-500">
            WinDivert
            <span class="text-mist-600">
              · {{ status.proxy.windivert.enabled ? status.proxy.windivert.state : 'disabled' }}
            </span>
          </span>
        </div>
        <p v-if="status.proxy.windivert.error" class="mt-1 text-[9.5px] leading-snug text-rose-400/80">
          {{ status.proxy.windivert.error }}
        </p>
      </div>
    </div>

    <div class="shrink-0 border-t border-ink-700/70 bg-ink-925/80 px-3 py-2.5">
      <div class="flex items-center justify-between gap-2">
        <div class="flex items-center gap-1.5 text-[10px] text-mist-500">
          <AppIcon name="database" :size="11" class="text-mist-600" />
          <span>{{ formatNumber(totalFlows) }} flows</span>
        </div>
        <span class="mono text-[10px] tabular-nums text-mist-500">
          {{ formatBytes(status?.store.bytes) }}
        </span>
      </div>
      <p
        class="mt-1 truncate font-mono text-[9px] text-mist-700"
        :title="status?.store.db_path"
      >
        {{ status?.store.db_path || 'No capture store connected' }}
      </p>
    </div>
  </aside>
</template>
