<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import AppIcon from './AppIcon.vue'
import type { FlowDetail } from '../types'
import {
  bodyBytes,
  bodyText,
  copyText,
  downloadText,
  flowUrl,
  formatBytes,
  looksBinary,
  shellQuote,
} from '../lib/format'
import { toastError } from '../composables/useToasts'

const props = defineProps<{ flow: FlowDetail }>()

const copied = ref(false)
let copiedTimer: number | null = null

const requestBody = computed(() => bodyText(props.flow.request, 512 * 1024))
const binaryBody = computed(() => {
  const bytes = bodyBytes(props.flow.request)
  return bytes.length > 0 && looksBinary(bytes)
})
const omittedBody = computed(
  () =>
    props.flow.request.body_size > 0 &&
    (binaryBody.value || requestBody.value == null),
)

const curlCommand = computed(() => {
  const headers = (props.flow.request.headers || []).filter((header) => {
    const name = header.name.toLowerCase()
    return (
      !name.startsWith(':') &&
      !['host', 'content-length', 'transfer-encoding', 'connection', 'accept-encoding'].includes(name)
    )
  })
  const parts: string[] = [
    `curl -X ${props.flow.method || 'GET'} ${shellQuote(flowUrl(props.flow))}`,
  ]
  if (props.flow.protocol?.toLowerCase().includes('grpc')) {
    parts.push('  --http2')
  }
  for (const header of headers) {
    parts.push(`  -H ${shellQuote(`${header.name}: ${header.value}`)}`)
  }
  parts.push('  --compressed')
  if (requestBody.value != null && !binaryBody.value && requestBody.value.length > 0) {
    parts.push(`  --data-raw ${shellQuote(requestBody.value)}`)
  }
  const command = parts.join(' \\\n')
  return omittedBody.value
    ? `# Binary request body (${formatBytes(props.flow.request.body_size)}) omitted from this command.\n${command}`
    : command
})

async function copyCommand(): Promise<void> {
  try {
    await copyText(curlCommand.value)
    copied.value = true
    if (copiedTimer != null) window.clearTimeout(copiedTimer)
    copiedTimer = window.setTimeout(() => {
      copied.value = false
    }, 1600)
  } catch (error) {
    toastError(error, 'Could not copy the cURL command')
  }
}

function downloadScript(): void {
  downloadText(`#!/usr/bin/env bash\n\n${curlCommand.value}\n`, 'mitmrs-request.sh', 'text/x-shellscript')
}

onBeforeUnmount(() => {
  if (copiedTimer != null) window.clearTimeout(copiedTimer)
})
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <div class="flex shrink-0 items-center gap-2 border-b border-ink-700/60 bg-ink-925/35 px-3 py-2">
      <div class="flex items-center gap-1.5">
        <AppIcon name="terminal" :size="12" class="text-accent-400/80" />
        <span class="text-[10.5px] font-semibold text-mist-400">cURL</span>
        <span class="rounded border border-ink-700 bg-ink-950/40 px-1 py-px font-mono text-[8.5px] text-mist-600">bash / zsh</span>
      </div>
      <div class="ml-auto flex items-center gap-1">
        <button class="icon-btn icon-btn-sm" type="button" title="Download as shell script" aria-label="Download shell script" @click="downloadScript">
          <AppIcon name="download" :size="13" />
        </button>
        <button class="btn btn-sm h-7 px-2" type="button" @click="copyCommand">
          <AppIcon :name="copied ? 'check' : 'copy'" :size="12" :class="copied ? 'text-emerald-400' : ''" />
          {{ copied ? 'Copied' : 'Copy' }}
        </button>
      </div>
    </div>

    <div class="app-scroll min-h-0 flex-1 p-3">
      <div class="overflow-hidden rounded-md border border-ink-700/80 bg-[var(--ui-sunken)]">
        <div class="flex items-center gap-1.5 border-b border-ink-700/50 px-2.5 py-1.5">
          <span class="h-2 w-2 rounded-full bg-rose-500/50" />
          <span class="h-2 w-2 rounded-full bg-amber-500/45" />
          <span class="h-2 w-2 rounded-full bg-emerald-500/45" />
          <span class="ml-1 font-mono text-[8.5px] text-mist-700">request.sh</span>
        </div>
        <pre class="mono app-scroll max-h-[min(52vh,520px)] overflow-auto whitespace-pre-wrap break-all px-3 py-3 text-[10.5px] leading-[1.7] text-[var(--ui-code-text-soft)]">{{ curlCommand }}</pre>
      </div>

      <div class="mt-2.5 flex items-start gap-1.5 text-[9.5px] leading-relaxed text-mist-700">
        <AppIcon name="info" :size="10" class="mt-0.5 shrink-0" />
        <span>
          Generated from the captured request. Cookie and authorization headers
          may contain live secrets — treat this command as sensitive.
        </span>
      </div>

      <div class="mt-3 grid grid-cols-2 gap-2">
        <div class="rounded-md border border-ink-700/60 bg-ink-950/25 px-2.5 py-2">
          <div class="text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">Request size</div>
          <div class="mono mt-0.5 text-[10.5px] text-mist-400">{{ formatBytes(flow.request.body_size) }}</div>
        </div>
        <div class="rounded-md border border-ink-700/60 bg-ink-950/25 px-2.5 py-2">
          <div class="text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">Headers</div>
          <div class="mono mt-0.5 text-[10.5px] text-mist-400">{{ flow.request.headers.length }}</div>
        </div>
      </div>
    </div>
  </div>
</template>
