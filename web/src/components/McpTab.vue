<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import AppIcon from './AppIcon.vue'
import type { FlowDetail } from '../types'
import { copyText, jsonStringify } from '../lib/format'
import { toastError } from '../composables/useToasts'

const props = defineProps<{
  flow: FlowDetail
  endpoint: string
}>()

const copiedKey = ref('')
let copiedTimer: number | null = null

const examples = computed(() => [
  {
    key: 'search',
    tool: 'traffic_search',
    description: 'Search captured flows before drilling into a specific request.',
    call: {
      jsonrpc: '2.0',
      id: 1,
      method: 'tools/call',
      params: {
        name: 'traffic_search',
        arguments: {
          q: props.flow.host,
          method: props.flow.method,
          limit: 20,
        },
      },
    },
  },
  {
    key: 'get',
    tool: 'traffic_get',
    description: 'Fetch the selected flow with headers and body content.',
    call: {
      jsonrpc: '2.0',
      id: 2,
      method: 'tools/call',
      params: {
        name: 'traffic_get',
        arguments: {
          id: props.flow.id,
          body: 'text',
        },
      },
    },
  },
  {
    key: 'decode',
    tool: 'traffic_decode',
    description: 'Decode the request payload using the auto-detecting decoder.',
    call: {
      jsonrpc: '2.0',
      id: 3,
      method: 'tools/call',
      params: {
        name: 'traffic_decode',
        arguments: {
          id: props.flow.id,
          direction: 'request',
          format: 'auto',
        },
      },
    },
  },
])

async function copyCall(key: string, payload: unknown): Promise<void> {
  try {
    await copyText(jsonStringify(payload))
    copiedKey.value = key
    if (copiedTimer != null) window.clearTimeout(copiedTimer)
    copiedTimer = window.setTimeout(() => {
      copiedKey.value = ''
    }, 1800)
  } catch (error) {
    toastError(error, 'Could not copy the tool call')
  }
}

async function copyEndpoint(): Promise<void> {
  try {
    await copyText(props.endpoint)
    copiedKey.value = 'endpoint'
    if (copiedTimer != null) window.clearTimeout(copiedTimer)
    copiedTimer = window.setTimeout(() => {
      copiedKey.value = ''
    }, 1800)
  } catch (error) {
    toastError(error, 'Could not copy the MCP endpoint')
  }
}

onBeforeUnmount(() => {
  if (copiedTimer != null) window.clearTimeout(copiedTimer)
})
</script>

<template>
  <div class="app-scroll h-full min-h-0 overflow-auto p-3">
    <div class="rounded-lg border border-accent-500/20 bg-accent-500/[0.035] p-3">
      <div class="flex items-start gap-2.5">
        <div class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-accent-500/25 bg-ink-950/50">
          <AppIcon name="sparkles" :size="14" class="text-accent-400" />
        </div>
        <div class="min-w-0">
          <h3 class="text-[11.5px] font-semibold text-mist-200">Give an AI agent this flow</h3>
          <p class="mt-0.5 text-[10px] leading-relaxed text-mist-500">
            MitmRs exposes captured traffic as MCP tools. An agent can search,
            read, and decode the same flows you see here, using the flow ID as
            context.
          </p>
          <div class="mt-2 flex items-center gap-1.5">
            <code class="mono min-w-0 truncate rounded border border-ink-700 bg-ink-950/60 px-1.5 py-1 text-[9px] text-accent-300/90">{{ endpoint }}</code>
            <button class="icon-btn icon-btn-tiny shrink-0" type="button" title="Copy MCP endpoint" aria-label="Copy MCP endpoint" @click="copyEndpoint">
              <AppIcon :name="copiedKey === 'endpoint' ? 'check' : 'copy'" :size="11" :class="copiedKey === 'endpoint' ? 'text-emerald-400' : ''" />
            </button>
          </div>
        </div>
      </div>
    </div>

    <div class="mt-3 flex items-center justify-between">
      <h4 class="section-title">Example tool calls</h4>
      <span class="font-mono text-[8.5px] text-mist-700">JSON-RPC · tools/call</span>
    </div>

    <div class="mt-2 space-y-2">
      <div
        v-for="example in examples"
        :key="example.key"
        class="overflow-hidden rounded-md border border-ink-700/75 bg-ink-950/30"
      >
        <div class="flex items-start gap-2 border-b border-ink-700/50 px-2.5 py-2">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-1.5">
              <span class="h-1 w-1 rounded-full bg-accent-500/70" />
              <span class="mono text-[10px] font-semibold text-accent-300/90">{{ example.tool }}</span>
            </div>
            <p class="mt-0.5 text-[9.5px] leading-snug text-mist-600">{{ example.description }}</p>
          </div>
          <button
            class="icon-btn icon-btn-tiny shrink-0"
            type="button"
            :title="`Copy ${example.tool} call`"
            :aria-label="`Copy ${example.tool} call`"
            @click="copyCall(example.key, example.call)"
          >
            <AppIcon :name="copiedKey === example.key ? 'check' : 'copy'" :size="11" :class="copiedKey === example.key ? 'text-emerald-400' : ''" />
          </button>
        </div>
        <pre class="mono app-scroll max-h-[190px] overflow-auto px-2.5 py-2 text-[9.5px] leading-[1.65] text-[var(--ui-code-text)]">{{ jsonStringify(example.call) }}</pre>
      </div>
    </div>

    <div class="mt-3 flex items-start gap-1.5 rounded-md border border-ink-700/60 bg-ink-950/25 px-2.5 py-2">
      <AppIcon name="info" :size="10" class="mt-0.5 shrink-0 text-mist-600" />
      <p class="text-[9px] leading-relaxed text-mist-700">
        Tool argument schemas are advertised by the MCP server. The selected
        flow ID
        <code class="mono break-all text-mist-500">{{ flow.id }}</code>
        is pre-filled for <span class="text-mist-500">traffic_get</span> and
        <span class="text-mist-500">traffic_decode</span>.
      </p>
    </div>
  </div>
</template>
