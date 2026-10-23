<script setup lang="ts">
import {
  computed,
  onBeforeUnmount,
  onMounted,
  reactive,
  ref,
  watch,
} from 'vue'
import TopBar from './components/TopBar.vue'
import AppIcon from './components/AppIcon.vue'
import FilterSidebar from './components/FilterSidebar.vue'
import FlowToolbar from './components/FlowToolbar.vue'
import FlowTable from './components/FlowTable.vue'
import FlowDetail from './components/FlowDetail.vue'
import StatusBar from './components/StatusBar.vue'
import ToastStack from './components/ToastStack.vue'
import ImportModal from './components/ImportModal.vue'
import ExportModal from './components/ExportModal.vue'
import CaModal from './components/CaModal.vue'
import McpInfoModal from './components/McpInfoModal.vue'
import SettingsModal from './components/SettingsModal.vue'
import Modal from './components/Modal.vue'
import type {
  FlowFilters,
  FlowSummary,
  ImportResult,
  ProxyAction,
  Stats,
  Status,
} from './types'
import { api, errorMessage, isAbortError, searchParams } from './composables/useApi'
import { useLive } from './composables/useLive'
import { usePanelResize } from './composables/usePanelResize'
import { toastError, toastInfo, toastSuccess } from './composables/useToasts'
import {
  activeFilterCount,
  copyText,
  matchesFilters,
  proxyAddress,
} from './lib/format'

type ModalName = 'import' | 'export' | 'ca' | 'mcp' | 'settings' | null
type PendingConfirm = { kind: 'clear' } | { kind: 'delete'; id: string; label: string }

const methodOptions = new Set([
  '',
  'GET',
  'POST',
  'PUT',
  'PATCH',
  'DELETE',
  'HEAD',
  'OPTIONS',
  'CONNECT',
])
const protocolOptions = new Set([
  '',
  'http/1.1',
  'http/2',
  'grpc',
  'sse',
  'ndjson',
  'websocket',
  'tls',
  'other',
])
const statusOptions = new Set(['', '2xx', '3xx', '4xx', '5xx', 'error'])
const directionOptions = new Set(['both', 'request', 'response'])

function initialFilters(): FlowFilters {
  const params = new URLSearchParams(window.location.search)
  const direction = params.get('direction') || 'both'
  const method = params.get('method') || ''
  const protocol = params.get('protocol') || ''
  const status = params.get('status') || ''
  return {
    q: params.get('q') || '',
    host: params.get('host') || '',
    method: methodOptions.has(method) ? method : '',
    status: statusOptions.has(status) ? status : '',
    protocol: protocolOptions.has(protocol) ? protocol : '',
    mime: params.get('mime') || '',
    body_contains: params.get('body_contains') || '',
    direction: directionOptions.has(direction)
      ? (direction as FlowFilters['direction'])
      : 'both',
  }
}

const filters = reactive<FlowFilters>(initialFilters())
const live = ref(new URLSearchParams(window.location.search).get('live') !== '0')
const status = ref<Status | null>(null)
const stats = ref<Stats | null>(null)
const flows = ref<FlowSummary[]>([])
const total = ref(0)
const page = ref(1)
const pageSize = ref(200)
const loading = ref(false)
const loadError = ref<string | null>(null)
const selectedId = ref<string | null>(null)
const selectedSummary = ref<FlowSummary | null>(null)
const detailRefreshKey = ref(0)
const activeModal = ref<ModalName>(null)
const pendingConfirm = ref<PendingConfirm | null>(null)
const confirmBusy = ref(false)
const sidebarOpen = ref(false)
const busyAction = ref<ProxyAction | null>(null)
const configBusy = ref(false)

// --- Resizable three-pane layout -------------------------------------------
// Every pane width is persisted in localStorage, clamped against the viewport,
// and driven by drag handles between the filter sidebar, flow table, and detail
// workspace. MAIN_MIN keeps the table usable when the detail pane is widened.
const MAIN_MIN = 420
const SIDEBAR_MIN = 200
const SIDEBAR_HARD_MAX = 460
const DETAIL_MIN = 360
const DETAIL_HARD_MAX = 1100
// Each splitter is a 7px hit area that overlaps its neighbours by 3px on both
// sides, so the pair still reserves 2px of layout width.
const SPLITTER_GUTTER = 2

const {
  width: sidebarWidth,
  dragging: sidebarDragging,
  start: startSidebarResize,
  reset: resetSidebarWidth,
  onKeydown: onSidebarResizeKeydown,
  commit: commitSidebarLayout,
} = usePanelResize({
  storageKey: 'mitmrs.layout.sidebar',
  min: SIDEBAR_MIN,
  hardMax: SIDEBAR_HARD_MAX,
  initial: () => 260,
  max: () => window.innerWidth - detailWidth.value - MAIN_MIN - SPLITTER_GUTTER,
  direction: 1,
})

const {
  width: detailWidth,
  dragging: detailDragging,
  start: startDetailResize,
  reset: resetDetailWidth,
  onKeydown: onDetailResizeKeydown,
  commit: commitDetailLayout,
} = usePanelResize({
  storageKey: 'mitmrs.layout.detail',
  min: DETAIL_MIN,
  hardMax: DETAIL_HARD_MAX,
  initial: () => Math.round(Math.min(640, Math.max(460, window.innerWidth * 0.36))),
  max: () => window.innerWidth - sidebarWidth.value - MAIN_MIN - SPLITTER_GUTTER,
  direction: -1,
})

const layoutStyle = computed(() => ({
  '--sidebar-w': `${sidebarWidth.value}px`,
  '--detail-w': `${detailWidth.value}px`,
}))

/**
 * Keeps the three panes inside the viewport after a reload or window resize.
 * The detail workspace yields width first (it is the flexible reading area),
 * then the filter sidebar, and the flow table keeps its minimum either way.
 */
function fitLayout(): void {
  if (window.innerWidth < 1280) return
  const budget = window.innerWidth - MAIN_MIN - SPLITTER_GUTTER
  let overflow = sidebarWidth.value + detailWidth.value - budget
  if (overflow <= 0) return
  const detailGive = Math.min(Math.max(0, detailWidth.value - DETAIL_MIN), overflow)
  if (detailGive > 0) {
    detailWidth.value -= detailGive
    overflow -= detailGive
  }
  if (overflow > 0) {
    sidebarWidth.value = Math.max(SIDEBAR_MIN, sidebarWidth.value - overflow)
  }
  commitDetailLayout()
  commitSidebarLayout()
}

let flowController: AbortController | null = null
let statusController: AbortController | null = null
let statsController: AbortController | null = null
let flowRequestId = 0
let filterTimer: number | null = null
let urlTimer: number | null = null
let liveRefreshTimer: number | null = null
let statsRefreshTimer: number | null = null
let detailRefreshTimer: number | null = null
let statusTimer: number | null = null

const activeFilters = computed(() => activeFilterCount(filters))
const proxyAddr = computed(() =>
  proxyAddress(
    status.value?.proxy.host || '127.0.0.1',
    status.value?.proxy.port || 8080,
  ),
)
const mcpEndpoint = computed(() => {  const value = status.value?.mcp.endpoint || '/mcp'
  if (/^https?:\/\//i.test(value)) return value
  try {
    return new URL(value, window.location.origin).toString()
  } catch {
    return `${window.location.origin}/mcp`
  }
})
const storedFlowCount = computed(
  () => status.value?.store.flows ?? stats.value?.total ?? total.value,
)
const pageCount = computed(() =>
  Math.max(1, Math.ceil(total.value / Math.max(1, pageSize.value))),
)
const selectedForDetail = computed(
  () =>
    flows.value.find((flow) => flow.id === selectedId.value) ??
    (selectedSummary.value?.id === selectedId.value ? selectedSummary.value : null),
)
const confirmTitle = computed(() =>
  pendingConfirm.value?.kind === 'clear' ? 'Clear all captured flows?' : 'Delete this flow?',
)
const confirmMessage = computed(() => {
  const pending = pendingConfirm.value
  if (!pending) return ''
  if (pending.kind === 'clear') {
    return `This permanently deletes ${storedFlowCount.value} captured flow(s) from the local SQLite database. The proxy and CA are not affected. Export first if you need to keep them.`
  }
  return `This permanently deletes ${pending.label} from the local capture database.`
})
function handleFlowEvent(flow: FlowSummary): void {
  if (!live.value) return
  if (page.value !== 1) {
    scheduleLiveRefresh()
    return
  }
  const index = flows.value.findIndex((item) => item.id === flow.id)
  if (index >= 0) {
    const next = [...flows.value]
    next[index] = flow
    flows.value = next
    if (selectedId.value === flow.id) {
      selectedSummary.value = flow
      scheduleDetailRefresh()
    }
  } else if (
    filters.body_contains.trim() === '' &&
    filters.direction === 'both' &&
    matchesFilters(flow, filters)
  ) {
    flows.value = [flow, ...flows.value].slice(0, pageSize.value)
    total.value += 1
    if (selectedId.value === flow.id) selectedSummary.value = flow
  }
  scheduleLiveRefresh()
  scheduleStatsRefresh()
}

function handleStatusEvent(next: Status): void {
  status.value = next
}

function fallbackPoll(): void {
  void loadFlows(true)
  void refreshStatus(true)
  void loadStats()
}

const liveControls = useLive({
  onFlow: handleFlowEvent,
  onStatus: handleStatusEvent,
  onPoll: fallbackPoll,
})

function scheduleLiveRefresh(): void {
  if (liveRefreshTimer != null) window.clearTimeout(liveRefreshTimer)
  liveRefreshTimer = window.setTimeout(() => {
    liveRefreshTimer = null
    if (live.value) void loadFlows(true)
  }, 650)
}

function scheduleStatsRefresh(): void {
  if (statsRefreshTimer != null) window.clearTimeout(statsRefreshTimer)
  statsRefreshTimer = window.setTimeout(() => {
    statsRefreshTimer = null
    void loadStats()
  }, 1200)
}

function scheduleDetailRefresh(): void {
  if (detailRefreshTimer != null) window.clearTimeout(detailRefreshTimer)
  detailRefreshTimer = window.setTimeout(() => {
    detailRefreshTimer = null
    detailRefreshKey.value += 1
  }, 500)
}

async function loadFlows(silent = false): Promise<void> {
  const requestId = ++flowRequestId
  flowController?.abort()
  const controller = new AbortController()
  flowController = controller
  if (!silent) loading.value = true
  try {
    const offset = (page.value - 1) * pageSize.value
    const response = await api.getFlows(filters, pageSize.value, offset, controller.signal)
    if (requestId !== flowRequestId) return
    flows.value = Array.isArray(response.items) ? response.items : []
    total.value = response.total ?? flows.value.length
    loadError.value = null
    const maxPage = Math.max(1, Math.ceil(total.value / Math.max(1, pageSize.value)))
    if (page.value > maxPage) {
      page.value = maxPage
      await loadFlows(silent)
      return
    }
  } catch (error) {
    if (requestId === flowRequestId && !isAbortError(error)) {
      loadError.value = errorMessage(error)
      if (!silent) toastError(error, 'Could not load traffic')
    }
  } finally {
    if (requestId === flowRequestId) {
      loading.value = false
      flowController = null
    }
  }
}

async function refreshStatus(silent = false): Promise<void> {
  statusController?.abort()
  const controller = new AbortController()
  statusController = controller
  try {
    const next = await api.getStatus(controller.signal)
    status.value = next
  } catch (error) {
    if (!isAbortError(error) && !silent) {
      toastError(error, 'Could not refresh proxy status')
    }
  } finally {
    if (statusController === controller) statusController = null
  }
}

async function loadStats(): Promise<void> {
  statsController?.abort()
  const controller = new AbortController()
  statsController = controller
  try {
    stats.value = await api.getStats(controller.signal)
  } catch (error) {
    if (!isAbortError(error)) {
      // Stats are supplementary; keep the last known snapshot.
    }
  } finally {
    if (statsController === controller) statsController = null
  }
}

async function manualRefresh(): Promise<void> {
  await Promise.all([loadFlows(false), refreshStatus(true), loadStats()])
  if (loadError.value) {
    toastError(loadError.value, 'Could not refresh traffic')
  } else {
    toastSuccess('Traffic refreshed', `${total.value.toLocaleString()} matching flows.`)
  }
}

function scheduleFilterLoad(): void {
  if (filterTimer != null) window.clearTimeout(filterTimer)
  filterTimer = window.setTimeout(() => {
    filterTimer = null
    page.value = 1
    closeDetail()
    void loadFlows()
  }, 260)
}

function scheduleUrlUpdate(): void {
  if (urlTimer != null) window.clearTimeout(urlTimer)
  urlTimer = window.setTimeout(() => {
    urlTimer = null
    const params = searchParams(filters)
    if (!live.value) params.set('live', '0')
    const query = params.toString()
    const nextUrl = `${window.location.pathname}${query ? `?${query}` : ''}`
    window.history.replaceState(null, '', nextUrl)
  }, 180)
}

function clearFilters(): void {
  page.value = 1
  Object.assign(filters, {
    q: '',
    host: '',
    method: '',
    status: '',
    protocol: '',
    mime: '',
    body_contains: '',
    direction: 'both',
  } satisfies FlowFilters)
}

function selectFlow(id: string): void {
  const summary = flows.value.find((flow) => flow.id === id) ?? null
  selectedSummary.value = summary
  selectedId.value = id
  sidebarOpen.value = false
}

function closeDetail(): void {
  selectedId.value = null
  selectedSummary.value = null
}

function toggleLive(): void {
  live.value = !live.value
  if (live.value) {
    liveControls.start()
    void loadFlows(true)
    toastInfo('Live updates resumed')
  } else {
    liveControls.stop()
    toastInfo('Live updates paused', 'Traffic is still being captured by the proxy.')
  }
  scheduleUrlUpdate()
}

async function onProxyAction(action: ProxyAction): Promise<void> {
  if (busyAction.value) return
  busyAction.value = action
  try {
    const next = await api.proxyAction(action)
    status.value = next
    const verb =
      action === 'start'
        ? 'Proxy started'
        : action === 'stop'
          ? 'Proxy stopped'
          : action === 'pause'
            ? 'Capture paused'
            : 'Capture resumed'
    toastSuccess(verb, action === 'start' ? `Listening on port ${next.proxy.port}.` : undefined)
    void loadStats()
    void loadFlows(true)
  } catch (error) {
    toastError(error, `Could not ${action} the proxy`)
    void refreshStatus(true)
  } finally {
    busyAction.value = null
  }
}

async function applyPort(port: number): Promise<void> {
  if (configBusy.value) return
  configBusy.value = true
  try {
    const next = await api.updateConfig({ port })
    status.value = next
    toastSuccess(
      'Proxy port updated',
      next.proxy.state === 'running'
        ? `Listener restarted on ${next.proxy.port}.`
        : `Next start will use port ${next.proxy.port}.`,
    )
    void loadStats()
  } catch (error) {
    toastError(error, 'Could not update proxy port')
  } finally {
    configBusy.value = false
  }
}

async function copyProxyAddress(): Promise<void> {
  try {
    await copyText(proxyAddr.value)
    toastSuccess('Proxy address copied', proxyAddr.value)
  } catch (error) {
    toastError(error, 'Could not copy proxy address')
  }
}

function openModal(name: Exclude<ModalName, null>): void {
  sidebarOpen.value = false
  activeModal.value = name
}

function requestClearFlows(): void {
  if (storedFlowCount.value === 0) {
    toastInfo('No captured flows to clear')
    return
  }
  pendingConfirm.value = { kind: 'clear' }
}

function requestDeleteSelected(): void {
  if (!selectedId.value) return
  const flow = selectedForDetail.value
  const label = flow
    ? `${flow.method} ${flow.host}${flow.path}`
    : 'the selected flow'
  pendingConfirm.value = { kind: 'delete', id: selectedId.value, label }
}

function closeConfirm(): void {
  if (!confirmBusy.value) pendingConfirm.value = null
}

async function runConfirm(): Promise<void> {
  const pending = pendingConfirm.value
  if (!pending || confirmBusy.value) return
  confirmBusy.value = true
  try {
    if (pending.kind === 'clear') {
      const result = await api.clearFlows()
      page.value = 1
      closeDetail()
      toastSuccess('Captured flows cleared', `${result.deleted} flow(s) deleted.`)
    } else {
      await api.deleteFlow(pending.id)
      if (selectedId.value === pending.id) closeDetail()
      toastSuccess('Flow deleted')
    }
    pendingConfirm.value = null
    await Promise.all([loadFlows(true), loadStats(), refreshStatus(true)])
  } catch (error) {
    toastError(error, pending.kind === 'clear' ? 'Could not clear flows' : 'Could not delete flow')
  } finally {
    confirmBusy.value = false
  }
}

function onImported(_result: ImportResult): void {
  page.value = 1
  void loadFlows(true)
  void loadStats()
  void refreshStatus(true)
}

function goToPage(next: number): void {
  const target = Math.max(1, Math.min(pageCount.value, Math.trunc(next)))
  if (target === page.value) return
  page.value = target
  closeDetail()
  void loadFlows()
}

function changePageSize(size: number): void {
  if (![100, 200, 500].includes(size) || size === pageSize.value) return
  pageSize.value = size
  page.value = 1
  closeDetail()
  void loadFlows()
}

function onStatusUpdated(next: Status): void {
  status.value = next
  void loadFlows(true)
  void loadStats()
}

function navigateFlow(delta: number): void {
  if (flows.value.length === 0) return
  const current = flows.value.findIndex((flow) => flow.id === selectedId.value)
  const index =
    current < 0
      ? delta > 0
        ? 0
        : flows.value.length - 1
      : Math.max(0, Math.min(flows.value.length - 1, current + delta))
  const flow = flows.value[index]
  if (flow) selectFlow(flow.id)
}

function isEditableTarget(target: EventTarget | null): boolean {
  const element = target as HTMLElement | null
  if (!element) return false
  const tag = element.tagName
  return (
    tag === 'INPUT' ||
    tag === 'TEXTAREA' ||
    tag === 'SELECT' ||
    element.isContentEditable
  )
}

function onGlobalKeydown(event: KeyboardEvent): void {
  if (activeModal.value) return
  if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey) return
  if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
  if (isEditableTarget(event.target)) return
  const target = event.target as HTMLElement | null
  if (target?.closest('.flow-scroll') || target?.closest('[role="dialog"]')) return
  event.preventDefault()
  navigateFlow(event.key === 'ArrowDown' ? 1 : -1)
}

function onVisibilityChange(): void {
  if (document.visibilityState === 'visible') {
    void refreshStatus(true)
    if (live.value) void loadFlows(true)
  }
}

watch(
  filters,
  () => {
    scheduleFilterLoad()
    scheduleUrlUpdate()
  },
  { deep: true },
)

watch(live, () => scheduleUrlUpdate())

onMounted(() => {
  void refreshStatus(true)
  void loadFlows()
  void loadStats()
  if (live.value) liveControls.start()
  fitLayout()
  window.addEventListener('resize', fitLayout)
  window.addEventListener('keydown', onGlobalKeydown)
  document.addEventListener('visibilitychange', onVisibilityChange)
  statusTimer = window.setInterval(() => {
    if (document.visibilityState === 'visible') void refreshStatus(true)
  }, 15_000)
})

onBeforeUnmount(() => {
  flowController?.abort()
  statusController?.abort()
  statsController?.abort()
  liveControls.stop()
  for (const timer of [
    filterTimer,
    urlTimer,
    liveRefreshTimer,
    statsRefreshTimer,
    detailRefreshTimer,
    statusTimer,
  ]) {
    if (timer != null) window.clearTimeout(timer)
  }
  window.removeEventListener('resize', fitLayout)
  window.removeEventListener('keydown', onGlobalKeydown)
  document.removeEventListener('visibilitychange', onVisibilityChange)
})
</script>

<template>
  <div class="app-canvas flex h-[100dvh] flex-col overflow-hidden">
    <TopBar
      :status="status"
      :busy-action="busyAction"
      :config-busy="configBusy"
      :connecting="!status"
      @start="onProxyAction('start')"
      @pause="onProxyAction('pause')"
      @resume="onProxyAction('resume')"
      @stop="onProxyAction('stop')"
      @apply-port="applyPort"
      @import="openModal('import')"
      @export="openModal('export')"
      @clear-flows="requestClearFlows"
      @ca="openModal('ca')"
      @mcp="openModal('mcp')"
      @settings="openModal('settings')"
      @toggle-sidebar="sidebarOpen = true"
    />

    <div class="app-main" :style="layoutStyle">
      <div class="pane-sidebar">
        <FilterSidebar
          :filters="filters"
          :stats="stats"
          :status="status"
          :total-flows="storedFlowCount"
          @clear="clearFilters"
          @resume="onProxyAction('resume')"
          @open-settings="openModal('settings')"
        />
      </div>

      <div
        class="splitter splitter-left"
        :class="{ 'is-active': sidebarDragging }"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize filters panel"
        :aria-valuenow="Math.round(sidebarWidth)"
        :aria-valuemin="SIDEBAR_MIN"
        :aria-valuemax="SIDEBAR_HARD_MAX"
        tabindex="0"
        title="Drag to resize · double-click to reset"
        @pointerdown="startSidebarResize"
        @dblclick="resetSidebarWidth"
        @keydown="onSidebarResizeKeydown"
      />

      <main class="flex min-h-0 min-w-0 flex-1 flex-col bg-ink-950/40">
        <FlowToolbar
          :visible-count="flows.length"
          :total="total"
          :loading="loading"
          :live="live"
          :transport="liveControls.transport.value"
          :active-filters="activeFilters"
          @refresh="manualRefresh"
          @toggle-live="toggleLive"
          @clear="clearFilters"
          @open-filters="sidebarOpen = true"
        />
        <FlowTable
          :flows="flows"
          :selected-id="selectedId"
          :total="total"
          :loading="loading"
          :has-filters="activeFilters > 0"
          :proxy-address="proxyAddr"
          :mcp-endpoint="mcpEndpoint"
          :proxy-state="status?.proxy.state ?? 'connecting'"
          :page="page"
          :page-size="pageSize"
          :page-count="pageCount"
          :error="flows.length ? null : loadError"
          @select="selectFlow"
          @clear="clearFilters"
          @refresh="manualRefresh"
          @page="goToPage"
          @page-size="changePageSize"
          @start="onProxyAction('start')"
          @copy-address="copyProxyAddress"
          @open-mcp="openModal('mcp')"
          @open-ca="openModal('ca')"
        />
      </main>

      <div
        class="splitter splitter-right"
        :class="{ 'is-active': detailDragging }"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize details panel"
        :aria-valuenow="Math.round(detailWidth)"
        :aria-valuemin="DETAIL_MIN"
        :aria-valuemax="DETAIL_HARD_MAX"
        tabindex="0"
        title="Drag to resize · double-click to reset"
        @pointerdown="startDetailResize"
        @dblclick="resetDetailWidth"
        @keydown="onDetailResizeKeydown"
      />

      <aside class="detail-shell border-l border-ink-700/80" :class="{ 'is-open': selectedId != null }">
        <FlowDetail
          :flow-id="selectedId"
          :summary="selectedForDetail"
          :refresh-key="detailRefreshKey"
          :mcp-endpoint="mcpEndpoint"
          @close="closeDetail"
          @delete="requestDeleteSelected"
        />
      </aside>
    </div>

    <div
      v-if="selectedId"
      class="fixed inset-0 z-[39] bg-black/60 backdrop-blur-[2px] xl:hidden"
      @click="closeDetail"
    />

    <Transition name="transition-fade">
      <div v-if="sidebarOpen" class="fixed inset-0 z-[60] lg:hidden">
        <div class="absolute inset-0 bg-black/65 backdrop-blur-[2px]" @click="sidebarOpen = false" />
        <div class="absolute inset-y-0 left-0 w-[min(320px,88vw)] border-r border-ink-700 bg-ink-925 shadow-2xl">
          <FilterSidebar
            :filters="filters"
            :stats="stats"
            :status="status"
            :total-flows="storedFlowCount"
            show-close
            @clear="clearFilters"
            @close="sidebarOpen = false"
            @resume="onProxyAction('resume')"
            @open-settings="openModal('settings')"
          />
        </div>
      </div>
    </Transition>

    <StatusBar
      :status="status"
      :live="live"
      :transport="liveControls.transport.value"
      :last-event-at="liveControls.lastEventAt.value"
      :total-flows="storedFlowCount"
      @open-mcp="openModal('mcp')"
    />

    <ToastStack />

    <ImportModal
      v-if="activeModal === 'import'"
      @close="activeModal = null"
      @imported="onImported"
    />
    <ExportModal
      v-if="activeModal === 'export'"
      :filters="filters"
      :total="total"
      @close="activeModal = null"
    />
    <CaModal v-if="activeModal === 'ca'" @close="activeModal = null" />
    <McpInfoModal
      v-if="activeModal === 'mcp'"
      :status="status"
      @close="activeModal = null"
    />
    <SettingsModal
      v-if="activeModal === 'settings'"
      :status="status"
      @close="activeModal = null"
      @updated="onStatusUpdated"
    />

    <Modal
      v-if="pendingConfirm"
      :title="confirmTitle"
      subtitle="This action cannot be undone"
      size="sm"
      :close-on-backdrop="!confirmBusy"
      @close="closeConfirm"
    >
      <p class="text-[11.5px] leading-relaxed text-mist-400">{{ confirmMessage }}</p>
      <template #footer>
        <button class="btn btn-sm" type="button" :disabled="confirmBusy" @click="closeConfirm">
          Cancel
        </button>
        <button
          class="btn btn-sm btn-danger"
          type="button"
          :disabled="confirmBusy"
          @click="runConfirm"
        >
          <AppIcon
            :name="confirmBusy ? 'refresh' : 'trash'"
            :size="12"
            :class="confirmBusy ? 'animate-soft-spin' : ''"
          />
          {{ confirmBusy ? 'Deleting…' : pendingConfirm.kind === 'clear' ? 'Clear all' : 'Delete flow' }}
        </button>
      </template>
    </Modal>
  </div>
</template>

<style scoped>
.app-main {
  position: relative;
  display: flex;
  min-height: 0;
  flex: 1;
  align-items: stretch;
  overflow: hidden;
}

.pane-sidebar {
  display: none;
  min-width: 0;
  flex: 0 0 auto;
  width: var(--sidebar-w, 260px);
}

/* Splitter handles overlap their neighbours slightly so the hit area is
   comfortable while only a hairline is visible at rest. */
.splitter {
  position: relative;
  z-index: 5;
  display: none;
  flex: 0 0 7px;
  width: 7px;
  margin: 0 -3px;
  cursor: col-resize;
  touch-action: none;
  outline: none;
}

.splitter::after {
  content: "";
  position: absolute;
  top: 0;
  bottom: 0;
  left: 50%;
  width: 1px;
  transform: translateX(-50%);
  border-radius: 999px;
  background: var(--ui-hairline);
  transition:
    width 120ms ease,
    background 120ms ease,
    box-shadow 120ms ease;
}

.splitter:hover::after,
.splitter:focus-visible::after,
.splitter.is-active::after {
  width: 2px;
  background: var(--ui-splitter-line);
  box-shadow: 0 0 10px var(--ui-splitter-glow);
}

.detail-shell {
  position: fixed;
  inset: 0;
  z-index: 40;
  display: flex;
  flex-direction: column;
  min-height: 0;
  visibility: hidden;
  transform: translateX(100%);
  background: var(--ui-panel);
  transition:
    transform 180ms ease,
    visibility 180ms ease;
}

.detail-shell.is-open {
  visibility: visible;
  transform: translateX(0);
}

@media (min-width: 1024px) {
  .pane-sidebar {
    display: flex;
  }

  .splitter-left {
    display: block;
  }
}

@media (min-width: 1280px) {
  .splitter-right {
    display: block;
  }

  .detail-shell {
    position: relative;
    inset: auto;
    z-index: auto;
    flex: 0 0 auto;
    width: var(--detail-w, 470px);
    visibility: visible;
    transform: none;
    transition: none;
  }
}
</style>
