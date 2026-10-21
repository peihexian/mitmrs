<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import AppIcon from './AppIcon.vue'
import Modal from './Modal.vue'
import type { ProxyConfigPatch, Status } from '../types'
import { api, errorMessage } from '../composables/useApi'
import { formatBytes, formatNumber } from '../lib/format'
import { toastError, toastInfo, toastSuccess } from '../composables/useToasts'

const props = defineProps<{ status: Status | null }>()
const emit = defineEmits<{
  close: []
  updated: [status: Status]
}>()

const port = ref(props.status ? String(props.status.proxy.port) : '8080')
const initialMaxBytes = props.status?.config?.max_body_bytes
const maxBodyMb = ref(
  initialMaxBytes != null
    ? Math.round((initialMaxBytes / (1024 * 1024)) * 10) / 10
    : 10,
)
const captureEnabled = ref(props.status?.config?.capture_enabled ?? true)
const saving = ref(false)
const restarting = ref(false)
const windivertBusy = ref<'start' | 'stop' | null>(null)
const error = ref('')
let cancelled = false

const supportsCaptureToggle = computed(
  () =>
    props.status?.config != null &&
    props.status.config.capture_enabled !== undefined,
)
const state = computed(() => props.status?.proxy.state ?? 'stopped')
const windivert = computed(() => props.status?.proxy.windivert ?? null)
const canRestart = computed(
  () => !restarting.value && !['starting', 'stopping'].includes(state.value),
)
const maxBytes = computed(() =>
  Number.isFinite(maxBodyMb.value) ? Math.round(maxBodyMb.value * 1024 * 1024) : 0,
)

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    window.setTimeout(resolve, ms)
  })
}

async function saveSettings(): Promise<void> {
  if (!props.status || saving.value) return
  error.value = ''
  const parsedPort = Number.parseInt(port.value, 10)
  if (!Number.isInteger(parsedPort) || parsedPort < 1 || parsedPort > 65535) {
    error.value = 'Enter a proxy port between 1 and 65535.'
    return
  }
  if (!Number.isFinite(maxBodyMb.value) || maxBodyMb.value <= 0 || maxBodyMb.value > 2048) {
    error.value = 'Max captured body must be between 0.1 MB and 2048 MB.'
    return
  }
  const patch: ProxyConfigPatch = {}
  if (parsedPort !== props.status.proxy.port) patch.port = parsedPort
  if (maxBytes.value !== props.status.config.max_body_bytes) {
    patch.max_body_bytes = maxBytes.value
  }
  if (
    supportsCaptureToggle.value &&
    captureEnabled.value !== props.status.config.capture_enabled
  ) {
    patch.capture_enabled = captureEnabled.value
  }
  if (Object.keys(patch).length === 0) {
    toastInfo('No changes', 'Settings already match the running configuration.')
    return
  }

  saving.value = true
  try {
    const next = await api.updateConfig(patch)
    emit('updated', next)
    if (patch.port != null) {
      port.value = String(next.proxy.port ?? parsedPort)
      toastSuccess(
        'Proxy port updated',
        `Listener configuration now uses port ${next.proxy.port ?? parsedPort}.`,
      )
    } else {
      toastSuccess('Settings applied')
    }
  } catch (saveError) {
    error.value = errorMessage(saveError)
    toastError(saveError, 'Could not update settings')
  } finally {
    saving.value = false
  }
}

async function controlWindivert(action: 'start' | 'stop'): Promise<void> {
  if (windivertBusy.value) return
  windivertBusy.value = action
  error.value = ''
  try {
    const next =
      action === 'start'
        ? await api.windivertStart()
        : await api.windivertStop()
    emit('updated', next)
    toastSuccess(
      action === 'start' ? 'WinDivert started' : 'WinDivert stopped',
      action === 'start'
        ? 'Transparent system-wide capture is active.'
        : 'Transparent capture is no longer redirecting traffic.',
    )
  } catch (windivertError) {
    error.value = errorMessage(windivertError)
    toastError(windivertError, `Could not ${action} WinDivert`)
  } finally {
    windivertBusy.value = null
  }
}

async function restartProxy(): Promise<void> {
  if (restarting.value) return
  restarting.value = true
  error.value = ''
  try {
    let current = await api.getStatus()
    if (current.proxy.state === 'running' || current.proxy.state === 'paused') {
      current = await api.proxyAction('stop')
      const deadline = Date.now() + 6000
      while (
        !cancelled &&
        Date.now() < deadline &&
        current.proxy.state !== 'stopped' &&
        current.proxy.state !== 'error'
      ) {
        await sleep(250)
        current = await api.getStatus()
      }
      if (current.proxy.state !== 'stopped' && current.proxy.state !== 'error') {
        throw new Error('The proxy did not stop within six seconds.')
      }
    }
    if (cancelled) return
    const started = await api.proxyAction('start')
    emit('updated', started)
    toastSuccess('Proxy restarted', `Listening on port ${started.proxy.port}.`)
  } catch (restartError) {
    error.value = errorMessage(restartError)
    toastError(restartError, 'Restart failed')
  } finally {
    restarting.value = false
  }
}

onBeforeUnmount(() => {
  cancelled = true
})
</script>

<template>
  <Modal
    title="Settings"
    subtitle="Listener and capture limits. Backend-owned values are read-only."
    size="md"
    @close="emit('close')"
  >
    <div class="flex items-center gap-2 rounded-md border border-ink-700/70 bg-ink-950/25 px-3 py-2">
      <span
        class="h-1.5 w-1.5 rounded-full"
        :class="
          state === 'running'
            ? 'bg-emerald-400'
            : state === 'paused'
              ? 'bg-amber-400'
              : state === 'error'
                ? 'bg-rose-400'
                : 'bg-mist-700'
        "
      />
      <span class="text-[10px] text-mist-400">
        Proxy
        <span class="capitalize text-mist-300">{{ state }}</span>
      </span>
      <span class="h-3 w-px bg-ink-700" />
      <span class="mono text-[9px] text-mist-600">
        {{ formatNumber(status?.proxy.active_connections ?? 0) }} active ·
        {{ formatNumber(status?.proxy.total_connections ?? 0) }} total
      </span>
    </div>

    <div class="mt-4 space-y-3">
      <div class="grid gap-3 sm:grid-cols-2">
        <div>
          <label class="field-label" for="settings-port">
            <span>Proxy port</span>
            <span class="normal-case tracking-normal text-mist-700">1–65535</span>
          </label>
          <input
            id="settings-port"
            v-model="port"
            type="text"
            inputmode="numeric"
            maxlength="5"
            class="field field-mono"
            :disabled="saving"
            @keydown.enter.prevent="saveSettings"
          />
          <p class="mt-1 text-[9px] leading-snug text-mist-700">
            Changing this while running restarts the listener.
          </p>
        </div>
        <div>
          <label class="field-label" for="settings-host">
            <span>Bind host</span>
            <span class="flex items-center gap-1 normal-case tracking-normal text-mist-700"><AppIcon name="lock" :size="8" /> backend</span>
          </label>
          <div class="field field-mono flex items-center text-mist-500">
            {{ status?.proxy.host || '127.0.0.1' }}
          </div>
          <p class="mt-1 text-[9px] leading-snug text-mist-700">
            The bind address is controlled by the Rust server configuration.
          </p>
        </div>
      </div>

      <div>
        <label class="field-label" for="settings-max-body">
          <span>Max captured body</span>
          <span class="normal-case tracking-normal text-mist-700">MB</span>
        </label>
        <div class="flex items-center gap-2">
          <input
            id="settings-max-body"
            v-model.number="maxBodyMb"
            type="number"
            min="0.1"
            max="2048"
            step="0.5"
            class="field field-mono max-w-[140px]"
            :disabled="saving"
          />
          <span class="mono text-[9.5px] text-mist-700">
            ≈ {{ formatBytes(maxBytes) }} per message
          </span>
        </div>
        <p class="mt-1 text-[9px] leading-snug text-mist-700">
          Larger bodies are stored truncated. Changes affect new captures;
          existing flow data is unchanged.
        </p>
      </div>

      <div
        v-if="supportsCaptureToggle"
        class="flex items-center justify-between gap-4 rounded-md border border-ink-700/70 bg-ink-950/25 px-3 py-2.5"
      >
        <div class="min-w-0">
          <div class="flex items-center gap-1.5">
            <AppIcon name="database" :size="12" class="text-mist-600" />
            <span class="text-[10.5px] font-medium text-mist-300">Capture enabled</span>
          </div>
          <p class="mt-0.5 text-[9px] leading-relaxed text-mist-600">
            When off, traffic can still proxy through, but new flows are not persisted.
          </p>
        </div>
        <button
          class="relative h-5 w-9 shrink-0 rounded-full transition"
          :class="captureEnabled ? 'bg-accent-500/80' : 'bg-ink-650'"
          type="button"
          role="switch"
          :aria-checked="captureEnabled"
          aria-label="Capture enabled"
          @click="captureEnabled = !captureEnabled"
        >
          <span
            class="absolute top-1 h-3 w-3 rounded-full bg-white shadow transition-all"
            :class="captureEnabled ? 'left-[21px]' : 'left-1'"
          />
        </button>
      </div>
    </div>

    <div
      v-if="windivert"
      class="mt-3 rounded-md border border-ink-700/70 bg-ink-950/25 px-3 py-2.5"
    >
      <div class="flex items-start justify-between gap-3">
        <div class="min-w-0">
          <div class="flex flex-wrap items-center gap-1.5">
            <AppIcon
              :name="windivert.enabled ? 'shield-check' : 'shield'"
              :size="12"
              :class="windivert.enabled ? 'text-teal-400' : 'text-mist-600'"
            />
            <span class="text-[10.5px] font-medium text-mist-300">WinDivert capture</span>
            <span
              class="badge"
              :class="
                windivert.enabled
                  ? 'border-teal-500/25 bg-teal-500/[0.06] text-teal-300'
                  : 'border-ink-700 bg-ink-800/50 text-mist-500'
              "
            >
              {{ windivert.enabled ? windivert.state || 'enabled' : 'disabled' }}
            </span>
            <span class="mono text-[8.5px] text-mist-700">
              {{ formatNumber(windivert.packets) }} packets
            </span>
          </div>
          <p class="mt-1 text-[9px] leading-relaxed text-mist-600">
            Transparent system-wide redirect for apps that ignore proxy
            settings. Requires Administrator and the WinDivert driver.
          </p>
          <p
            v-if="windivert.error"
            class="mt-1 break-words font-mono text-[9px] leading-snug text-rose-400/80"
          >
            {{ windivert.error }}
          </p>
        </div>
        <div class="flex shrink-0 items-center gap-1">
          <button
            class="btn btn-sm h-7 px-2"
            type="button"
            :disabled="windivertBusy != null || windivert.enabled"
            title="Start WinDivert (Administrator required)"
            @click="controlWindivert('start')"
          >
            <AppIcon
              :name="windivertBusy === 'start' ? 'refresh' : 'play'"
              :size="11"
              :class="windivertBusy === 'start' ? 'animate-soft-spin' : ''"
            />
            Start
          </button>
          <button
            class="btn btn-sm h-7 px-2"
            type="button"
            :disabled="windivertBusy != null || !windivert.enabled"
            title="Stop WinDivert"
            @click="controlWindivert('stop')"
          >
            <AppIcon
              :name="windivertBusy === 'stop' ? 'refresh' : 'stop'"
              :size="10"
              :class="windivertBusy === 'stop' ? 'animate-soft-spin' : ''"
            />
            Stop
          </button>
        </div>
      </div>
    </div>

    <div class="mt-5 border-t border-ink-700/60 pt-4">
      <div class="flex items-start justify-between gap-3">
        <div class="min-w-0">
          <h3 class="text-[10.5px] font-semibold text-mist-300">Restart listener</h3>
          <p class="mt-0.5 text-[9px] leading-relaxed text-mist-600">
            Stops and starts the proxy with the current backend configuration.
            Unsaved edits above are not applied.
          </p>
          <p
            v-if="status?.proxy.error"
            class="mt-1 break-words font-mono text-[9px] leading-snug text-rose-400/80"
          >
            {{ status.proxy.error }}
          </p>
        </div>
        <button
          class="btn btn-sm shrink-0"
          type="button"
          :disabled="!canRestart"
          @click="restartProxy"
        >
          <AppIcon name="refresh" :size="12" :class="restarting ? 'animate-soft-spin' : ''" />
          {{ restarting ? 'Restarting…' : 'Restart' }}
        </button>
      </div>
    </div>

    <div v-if="error" class="mt-3 flex items-start gap-1.5 rounded border border-rose-500/20 bg-rose-500/[0.045] px-2.5 py-2">
      <AppIcon name="alert-triangle" :size="11" class="mt-0.5 shrink-0 text-rose-400" />
      <p class="break-words text-[10px] leading-relaxed text-rose-300/80">{{ error }}</p>
    </div>

    <p class="mt-3 flex items-start gap-1.5 text-[9px] leading-relaxed text-mist-700">
      <AppIcon name="info" :size="10" class="mt-0.5 shrink-0" />
      WinDivert applies to the whole machine. Starting it requires an elevated
      backend process and the WinDivert driver.
    </p>

    <template #footer>
      <button class="btn btn-sm" type="button" :disabled="saving || restarting" @click="emit('close')">
        Cancel
      </button>
      <button class="btn btn-sm btn-primary" type="button" :disabled="saving || restarting || !status" @click="saveSettings">
        <AppIcon :name="saving ? 'refresh' : 'check'" :size="12" :class="saving ? 'animate-soft-spin' : ''" />
        {{ saving ? 'Saving…' : 'Save settings' }}
      </button>
    </template>
  </Modal>
</template>
