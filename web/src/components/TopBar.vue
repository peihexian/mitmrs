<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import AppIcon from './AppIcon.vue'
import type { ProxyAction, ProxyState, Status } from '../types'
import { useTheme } from '../composables/useTheme'

const { theme, toggleTheme } = useTheme()
const themeToggleLabel = computed(() =>
  theme.value === 'light' ? 'Switch to dark theme' : 'Switch to light theme',
)

const props = withDefaults(
  defineProps<{
    status: Status | null
    busyAction: ProxyAction | null
    connecting: boolean
    configBusy: boolean
  }>(),
  { connecting: false, configBusy: false },
)

const emit = defineEmits<{
  start: []
  pause: []
  resume: []
  stop: []
  'apply-port': [port: number]
  import: []
  export: []
  'clear-flows': []
  ca: []
  mcp: []
  settings: []
  'toggle-sidebar': []
}>()

const state = computed<ProxyState | 'connecting'>(() =>
  props.status ? props.status.proxy.state : 'connecting',
)
const portDraft = ref(props.status ? String(props.status.proxy.port) : '')
const portFocused = ref(false)
const portError = ref('')

watch(
  () => props.status?.proxy.port,
  (port) => {
    if (port != null && !portFocused.value) portDraft.value = String(port)
  },
)

const stateInfo = computed(() => {
  switch (state.value) {
    case 'running':
      return {
        label: 'Proxy running',
        short: 'Running',
        dot: 'bg-emerald-400',
        text: 'text-emerald-300',
        border: 'border-emerald-500/25',
        bg: 'bg-emerald-500/[0.08]',
      }
    case 'paused':
      return {
        label: 'Capture paused',
        short: 'Paused',
        dot: 'bg-amber-400',
        text: 'text-amber-300',
        border: 'border-amber-500/25',
        bg: 'bg-amber-500/[0.08]',
      }
    case 'starting':
      return {
        label: 'Proxy starting',
        short: 'Starting',
        dot: 'bg-sky-400 animate-pulse',
        text: 'text-sky-300',
        border: 'border-sky-500/25',
        bg: 'bg-sky-500/[0.08]',
      }
    case 'stopping':
      return {
        label: 'Proxy stopping',
        short: 'Stopping',
        dot: 'bg-sky-400 animate-pulse',
        text: 'text-sky-300',
        border: 'border-sky-500/25',
        bg: 'bg-sky-500/[0.08]',
      }
    case 'error':
      return {
        label: props.status?.proxy.error || 'Proxy error',
        short: 'Error',
        dot: 'bg-rose-400',
        text: 'text-rose-300',
        border: 'border-rose-500/25',
        bg: 'bg-rose-500/[0.08]',
      }
    case 'stopped':
      return {
        label: 'Proxy stopped',
        short: 'Stopped',
        dot: 'bg-mist-600',
        text: 'text-mist-400',
        border: 'border-ink-700',
        bg: 'bg-ink-800/60',
      }
    default:
      return {
        label: 'Connecting to backend',
        short: 'Connecting',
        dot: 'bg-mist-600 animate-pulse',
        text: 'text-mist-500',
        border: 'border-ink-700',
        bg: 'bg-ink-800/60',
      }
  }
})

const canStart = computed(() => state.value === 'stopped' || state.value === 'error')
const canPause = computed(() => state.value === 'running')
const canResume = computed(() => state.value === 'paused')
const canStop = computed(
  () => state.value === 'running' || state.value === 'paused' || state.value === 'starting',
)

function applyPort(): void {
  const port = Number.parseInt(portDraft.value.trim(), 10)
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    portError.value = 'Enter a port between 1 and 65535.'
    return
  }
  portError.value = ''
  emit('apply-port', port)
}

function onPortBlur(): void {
  portFocused.value = false
  if (props.status) portDraft.value = String(props.status.proxy.port)
  portError.value = ''
}
</script>

<template>
  <header
    class="relative z-30 shrink-0 border-b border-ink-700/80 bg-ink-925/90 backdrop-blur-xl"
  >
    <div class="flex h-[56px] items-center gap-2 px-2 sm:gap-3 sm:px-3">
      <button
        class="icon-btn lg:hidden"
        type="button"
        title="Open filters"
        aria-label="Open filters"
        @click="emit('toggle-sidebar')"
      >
        <AppIcon name="panel-left" :size="17" />
      </button>

      <div class="flex min-w-0 shrink-0 items-center gap-2.5">
        <div
          class="relative flex h-8 w-8 shrink-0 items-center justify-center overflow-hidden rounded-lg border border-accent-500/30 bg-[#07141a] shadow-[inset_0_0_16px_rgba(34,211,238,0.08)]"
        >
          <svg viewBox="0 0 32 32" class="h-6 w-6" aria-hidden="true">
            <path
              d="M4 21h4.5l3.2-9.5 3.8 12 3.2-8.5 2.1 4H28"
              fill="none"
              stroke="#22d3ee"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
            <circle cx="25.7" cy="19" r="1.6" fill="#2dd4bf" />
          </svg>
          <div
            class="pointer-events-none absolute inset-x-0 bottom-0 h-px bg-gradient-to-r from-transparent via-accent-400/60 to-transparent"
          />
        </div>
        <div class="hidden min-w-0 xs:block">
          <div class="flex items-baseline gap-1.5">
            <span class="text-[16px] font-bold leading-none tracking-[-0.025em] text-mist-200">
              Mitm<span class="text-accent-400">Rs</span>
            </span>
            <span
              class="hidden rounded border border-ink-700 px-1 py-px text-[8px] font-semibold uppercase leading-none tracking-[0.12em] text-mist-600 xl:inline"
            >lab</span>
          </div>
          <p
            class="mt-0.5 hidden truncate text-[9.5px] font-medium uppercase tracking-[0.08em] text-mist-600 sm:block"
          >
            AI-native MITM traffic lab
          </p>
        </div>
      </div>

      <div
        class="ml-1 flex h-[26px] shrink-0 items-center gap-1.5 rounded-full border px-2 sm:ml-2"
        :class="[stateInfo.border, stateInfo.bg]"
        :title="stateInfo.label + (status?.proxy.error ? ` — ${status.proxy.error}` : '')"
      >
        <span class="h-1.5 w-1.5 rounded-full" :class="stateInfo.dot" />
        <span class="text-[10px] font-semibold tracking-wide" :class="stateInfo.text">
          {{ stateInfo.short }}
        </span>
        <span
          v-if="status"
          class="mono hidden text-[9.5px] text-mist-600 md:inline"
        >:{{ status.proxy.port }}</span>
      </div>

      <div class="ml-auto hidden items-center gap-1.5 lg:flex">
        <div class="flex items-center">
          <div
            class="flex h-8 overflow-hidden rounded-md border border-ink-700 bg-ink-950/50 focus-within:border-accent-500/40"
            :title="portError || 'Listener port (applies immediately; restarts a running proxy)'"
          >
            <span
              class="flex items-center border-r border-ink-700/70 bg-ink-850/50 px-2 text-[9px] font-semibold uppercase tracking-[0.08em] text-mist-600"
            >Port</span>
            <input
              v-model="portDraft"
              type="text"
              inputmode="numeric"
              maxlength="5"
              class="mono w-[58px] bg-transparent px-1.5 text-[11.5px] text-mist-300 outline-none placeholder:text-mist-700"
              aria-label="Proxy port"
              :class="portError ? 'text-rose-400' : ''"
              @focus="portFocused = true"
              @blur="onPortBlur"
              @keydown.enter.prevent="applyPort"
            />
          </div>
          <button
            class="btn btn-sm ml-1 border-ink-700 px-2"
            type="button"
            :disabled="busyAction != null || connecting || configBusy"
            title="Apply proxy port"
            @click="applyPort"
          >
            <AppIcon
              :name="configBusy ? 'refresh' : 'check'"
              :size="13"
              :class="configBusy ? 'animate-soft-spin' : ''"
            />
            <span class="hidden xl:inline">Apply</span>
          </button>
        </div>

        <div class="mx-0.5 h-5 w-px bg-ink-700/80" />

        <button
          v-if="canStart"
          class="btn btn-sm btn-primary"
          type="button"
          :disabled="busyAction != null || connecting"
          @click="emit('start')"
        >
          <AppIcon
            :name="busyAction === 'start' ? 'refresh' : 'play'"
            :size="13"
            :class="busyAction === 'start' ? 'animate-soft-spin' : ''"
          />
          Start
        </button>
        <button
          v-if="canPause"
          class="btn btn-sm"
          type="button"
          :disabled="busyAction != null"
          @click="emit('pause')"
        >
          <AppIcon
            :name="busyAction === 'pause' ? 'refresh' : 'pause'"
            :size="13"
            :class="busyAction === 'pause' ? 'animate-soft-spin' : ''"
          />
          Pause
        </button>
        <button
          v-if="canResume"
          class="btn btn-sm btn-primary"
          type="button"
          :disabled="busyAction != null"
          @click="emit('resume')"
        >
          <AppIcon
            :name="busyAction === 'resume' ? 'refresh' : 'play'"
            :size="13"
            :class="busyAction === 'resume' ? 'animate-soft-spin' : ''"
          />
          Resume
        </button>
        <button
          v-if="canStop"
          class="btn btn-sm"
          type="button"
          :disabled="busyAction != null"
          @click="emit('stop')"
        >
          <AppIcon
            :name="busyAction === 'stop' ? 'refresh' : 'stop'"
            :size="12"
            :class="busyAction === 'stop' ? 'animate-soft-spin' : ''"
          />
          Stop
        </button>

        <div class="mx-0.5 h-5 w-px bg-ink-700/80" />

        <button class="btn btn-sm btn-ghost px-2" type="button" title="Import traffic" @click="emit('import')">
          <AppIcon name="upload" :size="14" />
          <span class="hidden xl:inline">Import</span>
        </button>
        <button class="btn btn-sm btn-ghost px-2" type="button" title="Export traffic" @click="emit('export')">
          <AppIcon name="download" :size="14" />
          <span class="hidden xl:inline">Export</span>
        </button>
        <button
          class="btn btn-sm btn-ghost px-2 text-rose-300/80 hover:text-rose-200"
          type="button"
          title="Clear all captured flows"
          :disabled="!status || status.store.flows === 0"
          @click="emit('clear-flows')"
        >
          <AppIcon name="trash" :size="14" />
          <span class="hidden xl:inline">Clear</span>
        </button>
        <button class="btn btn-sm btn-ghost px-2" type="button" title="CA certificate" @click="emit('ca')">
          <AppIcon name="shield-check" :size="14" />
          <span class="hidden xl:inline">CA</span>
        </button>
        <button
          class="btn btn-sm btn-ghost px-2 text-accent-300/90 hover:text-accent-200"
          type="button"
          title="MCP / AI integration"
          @click="emit('mcp')"
        >
          <AppIcon name="sparkles" :size="14" />
          <span class="hidden xl:inline">MCP</span>
        </button>
        <button
          class="icon-btn"
          type="button"
          :title="themeToggleLabel"
          :aria-label="themeToggleLabel"
          @click="toggleTheme"
        >
          <AppIcon :name="theme === 'light' ? 'moon' : 'sun'" :size="15" />
        </button>
        <button class="icon-btn" type="button" title="Settings" aria-label="Settings" @click="emit('settings')">
          <AppIcon name="settings" :size="15" />
        </button>
      </div>

      <div class="ml-auto flex items-center gap-1 lg:hidden">
        <button
          v-if="canStart"
          class="btn btn-sm btn-primary px-2.5"
          type="button"
          :disabled="busyAction != null || connecting"
          @click="emit('start')"
        >
          <AppIcon name="play" :size="13" />
          Start
        </button>
        <button
          v-if="canPause"
          class="btn btn-sm px-2"
          type="button"
          :disabled="busyAction != null"
          aria-label="Pause proxy"
          @click="emit('pause')"
        >
          <AppIcon name="pause" :size="14" />
        </button>
        <button
          v-if="canResume"
          class="btn btn-sm btn-primary px-2"
          type="button"
          :disabled="busyAction != null"
          aria-label="Resume proxy"
          @click="emit('resume')"
        >
          <AppIcon name="play" :size="14" />
        </button>
        <button
          v-if="canStop"
          class="btn btn-sm px-2"
          type="button"
          :disabled="busyAction != null"
          aria-label="Stop proxy"
          @click="emit('stop')"
        >
          <AppIcon name="stop" :size="12" />
        </button>
        <button
          class="icon-btn"
          type="button"
          :title="themeToggleLabel"
          :aria-label="themeToggleLabel"
          @click="toggleTheme"
        >
          <AppIcon :name="theme === 'light' ? 'moon' : 'sun'" :size="16" />
        </button>
        <button class="icon-btn" type="button" aria-label="Settings" @click="emit('settings')">
          <AppIcon name="settings" :size="16" />
        </button>
      </div>
    </div>

    <div
      class="flex items-center gap-1 overflow-x-auto border-t border-ink-700/50 px-2 py-1.5 lg:hidden"
    >
      <div
        class="flex h-7 shrink-0 overflow-hidden rounded-md border border-ink-700 bg-ink-950/50"
      >
        <span
          class="flex items-center border-r border-ink-700/70 px-1.5 text-[8.5px] font-semibold uppercase tracking-wider text-mist-600"
        >Port</span>
        <input
          v-model="portDraft"
          type="text"
          inputmode="numeric"
          maxlength="5"
          class="mono w-[54px] bg-transparent px-1 text-[11px] text-mist-300 outline-none"
          aria-label="Proxy port"
          @focus="portFocused = true"
          @blur="onPortBlur"
          @keydown.enter.prevent="applyPort"
        />
        <button
          class="flex w-7 items-center justify-center border-l border-ink-700/70 text-mist-500 active:bg-ink-800"
          type="button"
          aria-label="Apply proxy port"
          :disabled="busyAction != null || configBusy"
          @click="applyPort"
        >
          <AppIcon :name="configBusy ? 'refresh' : 'check'" :size="12" :class="configBusy ? 'animate-soft-spin' : ''" />
        </button>
      </div>
      <button class="btn btn-sm btn-ghost shrink-0 px-2" type="button" title="Import" @click="emit('import')">
        <AppIcon name="upload" :size="14" />
      </button>
      <button class="btn btn-sm btn-ghost shrink-0 px-2" type="button" title="Export" @click="emit('export')">
        <AppIcon name="download" :size="14" />
      </button>
      <button
        class="btn btn-sm btn-ghost shrink-0 px-2 text-rose-300/80"
        type="button"
        title="Clear all captured flows"
        :disabled="!status || status.store.flows === 0"
        @click="emit('clear-flows')"
      >
        <AppIcon name="trash" :size="14" />
      </button>
      <button class="btn btn-sm btn-ghost shrink-0 px-2" type="button" title="CA certificate" @click="emit('ca')">
        <AppIcon name="shield-check" :size="14" />
      </button>
      <button
        class="btn btn-sm btn-ghost shrink-0 px-2 text-accent-300/90"
        type="button"
        title="MCP"
        @click="emit('mcp')"
      >
        <AppIcon name="sparkles" :size="14" />
      </button>
      <span
        v-if="status?.proxy.error"
        class="ml-1 truncate text-[10px] text-rose-400"
        :title="status.proxy.error"
      >{{ status.proxy.error }}</span>
    </div>
  </header>
</template>
