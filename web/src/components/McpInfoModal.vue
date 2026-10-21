<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import AppIcon from './AppIcon.vue'
import Modal from './Modal.vue'
import CopyBlock from './CopyBlock.vue'
import type { McpInfo, McpTool, Status } from '../types'
import { api, errorMessage, isAbortError } from '../composables/useApi'
import { copyText } from '../lib/format'
import { toastError } from '../composables/useToasts'

const props = defineProps<{ status: Status | null }>()
const emit = defineEmits<{ close: [] }>()

const info = ref<McpInfo | null>(null)
const loading = ref(true)
const error = ref('')
const copiedEndpoint = ref(false)
let controller: AbortController | null = null
let copyTimer: number | null = null

const endpoint = computed(
  () => info.value?.endpoint || props.status?.mcp.endpoint || '/mcp',
)
const absoluteEndpoint = computed(() => {
  const value = (endpoint.value || '/mcp').trim()
  if (/^https?:\/\//i.test(value)) return value
  try {
    return new URL(value, window.location.origin).toString()
  } catch {
    return `${window.location.origin}/mcp`
  }
})
const stdioCommand = computed(
  () => info.value?.stdio_command || props.status?.mcp.stdio_command || 'mitmrs mcp --stdio',
)
const tools = computed<McpTool[]>(() => {
  if (info.value?.tools?.length) return info.value.tools
  return (props.status?.mcp.tools ?? []).map((name) => ({
    name,
    description: '',
  }))
})

const claudeCommand = computed(
  () => `claude mcp add --transport http mitmrs ${absoluteEndpoint.value}`,
)
const claudeConfig = computed(() =>
  JSON.stringify(
    {
      mcpServers: {
        mitmrs: {
          type: 'http',
          url: absoluteEndpoint.value,
        },
      },
    },
    null,
    2,
  ),
)
const cursorConfig = computed(() =>
  JSON.stringify(
    {
      mcpServers: {
        mitmrs: {
          url: absoluteEndpoint.value,
        },
      },
    },
    null,
    2,
  ),
)

async function loadInfo(): Promise<void> {
  controller?.abort()
  controller = new AbortController()
  loading.value = true
  error.value = ''
  try {
    info.value = await api.getMcpInfo(controller.signal)
  } catch (loadError) {
    if (!isAbortError(loadError)) {
      error.value = errorMessage(loadError)
    }
  } finally {
    loading.value = false
  }
}

async function copyEndpoint(): Promise<void> {
  try {
    await copyText(absoluteEndpoint.value)
    copiedEndpoint.value = true
    if (copyTimer != null) window.clearTimeout(copyTimer)
    copyTimer = window.setTimeout(() => {
      copiedEndpoint.value = false
    }, 1600)
  } catch (copyError) {
    toastError(copyError, 'Could not copy the endpoint')
  }
}

onMounted(loadInfo)
onBeforeUnmount(() => {
  controller?.abort()
  if (copyTimer != null) window.clearTimeout(copyTimer)
})
</script>

<template>
  <Modal
    title="MCP / AI integration"
    subtitle="Let Claude, Cursor, and other MCP clients inspect the live capture."
    size="lg"
    @close="emit('close')"
  >
    <div class="overflow-hidden rounded-lg border border-accent-500/20 bg-accent-500/[0.035]">
      <div class="flex items-start gap-3 p-3.5">
        <div class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-accent-500/25 bg-ink-950/50">
          <AppIcon name="sparkles" :size="19" class="text-accent-400" />
        </div>
        <div class="min-w-0 flex-1">
          <div class="flex flex-wrap items-center gap-2">
            <h3 class="text-[12px] font-semibold text-mist-200">Model Context Protocol server</h3>
            <span v-if="loading" class="flex items-center gap-1 text-[8.5px] text-mist-600">
              <AppIcon name="refresh" :size="9" class="animate-soft-spin" /> loading
            </span>
            <span v-else-if="error" class="badge border-amber-500/20 bg-amber-500/[0.05] text-amber-300/80">Fallback</span>
            <span v-else class="badge border-emerald-500/20 bg-emerald-500/[0.05] text-emerald-300/80">Available</span>
          </div>
          <p class="mt-1 text-[10px] leading-relaxed text-mist-500">
            Agents can search traffic, read complete flows, and decode payloads
            through tools — with the same filters and stored captures as this UI.
          </p>
          <div class="mt-2 flex items-center gap-1.5">
            <code class="mono min-w-0 flex-1 truncate rounded border border-ink-700 bg-ink-950/60 px-2 py-1.5 text-[9.5px] text-accent-300/90">{{ absoluteEndpoint }}</code>
            <button class="icon-btn icon-btn-sm shrink-0" type="button" :title="copiedEndpoint ? 'Copied' : 'Copy endpoint'" aria-label="Copy MCP endpoint" @click="copyEndpoint">
              <AppIcon :name="copiedEndpoint ? 'check' : 'copy'" :size="12" :class="copiedEndpoint ? 'text-emerald-400' : ''" />
            </button>
          </div>
        </div>
      </div>
    </div>

    <div
      v-if="error"
      class="mt-2.5 flex items-start gap-2 rounded-md border border-amber-500/20 bg-amber-500/[0.04] px-2.5 py-2"
    >
      <AppIcon name="alert-triangle" :size="11" class="mt-0.5 shrink-0 text-amber-400" />
      <div class="min-w-0 flex-1">
        <p class="text-[10px] leading-relaxed text-amber-200/70">
          MCP details could not be refreshed: {{ error }}
        </p>
        <button class="mt-1 text-[9.5px] font-semibold text-amber-300 hover:text-amber-200" type="button" @click="loadInfo">
          Retry
        </button>
      </div>
    </div>

    <div class="mt-3 grid gap-2 sm:grid-cols-2">
      <div class="rounded-md border border-ink-700/70 bg-ink-950/25 p-2.5">
        <div class="flex items-center gap-1.5">
          <AppIcon name="globe" :size="11" class="text-accent-500/70" />
          <span class="text-[9px] font-semibold uppercase tracking-[0.08em] text-mist-600">HTTP transport</span>
        </div>
        <p class="mono mt-1 truncate text-[9.5px] text-mist-400" :title="absoluteEndpoint">POST {{ endpoint }}</p>
      </div>
      <div class="rounded-md border border-ink-700/70 bg-ink-950/25 p-2.5">
        <div class="flex items-center gap-1.5">
          <AppIcon name="terminal" :size="11" class="text-teal-500/70" />
          <span class="text-[9px] font-semibold uppercase tracking-[0.08em] text-mist-600">stdio transport</span>
        </div>
        <p class="mono mt-1 truncate text-[9.5px] text-mist-400" :title="stdioCommand">{{ stdioCommand }}</p>
      </div>
    </div>

    <div class="mt-4">
      <div class="flex items-center justify-between">
        <h3 class="section-title">Claude Code</h3>
        <span class="font-mono text-[8.5px] text-mist-700">CLI · HTTP</span>
      </div>
      <div class="mt-1.5 space-y-1.5">
        <CopyBlock :code="claudeCommand" label="Add the server" language="bash" max-height="70px" />
        <CopyBlock :code="claudeConfig" label="Project .mcp.json (alternative)" language="json" max-height="170px" />
      </div>
    </div>

    <div class="mt-4">
      <div class="flex items-center justify-between">
        <h3 class="section-title">Cursor</h3>
        <span class="font-mono text-[8.5px] text-mist-700">%USERPROFILE%\.cursor\mcp.json</span>
      </div>
      <div class="mt-1.5">
        <CopyBlock :code="cursorConfig" label="Cursor MCP configuration" language="json" max-height="150px" />
      </div>
    </div>

    <div class="mt-4">
      <div class="flex items-center justify-between">
        <h3 class="section-title">Available tools</h3>
        <span class="font-mono text-[8.5px] text-mist-700">{{ tools.length }} tools</span>
      </div>
      <div class="mt-1.5 overflow-hidden rounded-md border border-ink-700/70 bg-ink-950/25">
        <div
          v-for="(tool, index) in tools"
          :key="tool.name"
          class="flex items-start gap-2.5 px-2.5 py-2"
          :class="index > 0 ? 'border-t border-ink-700/40' : ''"
        >
          <span class="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded border border-ink-700 bg-ink-900">
            <AppIcon name="wand" :size="10" class="text-accent-500/70" />
          </span>
          <div class="min-w-0">
            <span class="mono text-[10px] font-semibold text-mist-300">{{ tool.name }}</span>
            <p v-if="tool.description" class="mt-0.5 text-[9.5px] leading-relaxed text-mist-600">
              {{ tool.description }}
            </p>
          </div>
        </div>
        <p v-if="tools.length === 0" class="px-3 py-3 text-[10px] text-mist-600">
          No tools reported by the backend.
        </p>
      </div>
    </div>

    <p class="mt-3 flex items-start gap-1.5 text-[9px] leading-relaxed text-mist-700">
      <AppIcon name="info" :size="10" class="mt-0.5 shrink-0" />
      Restart your MCP client after updating its configuration. The server is
      same-origin and only listens on your local MitmRs endpoint.
    </p>

    <template #footer>
      <button class="btn btn-sm" type="button" @click="emit('close')">Close</button>
    </template>
  </Modal>
</template>
