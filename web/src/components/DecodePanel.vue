<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import AppIcon from './AppIcon.vue'
import DecodeTree from './DecodeTree.vue'
import type { DecodeResult, DecoderInfo, FlowDetail } from '../types'
import { api, isAbortError } from '../composables/useApi'
import { copyText, formatBytes, jsonStringify } from '../lib/format'
import { toastError } from '../composables/useToasts'

const props = defineProps<{ flow: FlowDetail }>()

const direction = ref<'request' | 'response'>('request')
const format = ref('auto')
const loading = ref(false)
const error = ref('')
const result = ref<DecodeResult | null>(null)
const decoders = ref<DecoderInfo[]>([])
const copied = ref(false)
let controller: AbortController | null = null
let decoderController: AbortController | null = null

const baseFormats = [
  { id: 'auto', name: 'Auto detect' },
  { id: 'http', name: 'HTTP' },
  { id: 'http2', name: 'HTTP/2' },
  { id: 'grpc', name: 'gRPC' },
  { id: 'protobuf', name: 'Protobuf' },
  { id: 'json', name: 'JSON' },
  { id: 'xml', name: 'XML' },
  { id: 'form', name: 'Form' },
  { id: 'multipart', name: 'Multipart' },
  { id: 'websocket', name: 'WebSocket' },
  { id: 'text', name: 'Text' },
  { id: 'base64', name: 'Base64' },
  { id: 'hex', name: 'Hex' },
]

const formats = computed(() => {
  const list = [...baseFormats]
  for (const decoder of decoders.value) {
    if (!list.some((item) => item.id === decoder.id)) {
      list.push({ id: decoder.id, name: decoder.name || decoder.id })
    }
  }
  return list
})

const flowIsGrpc = computed(() => {
  const protocol = (props.flow.protocol || '').toLowerCase()
  const mime = `${props.flow.request_mime || ''} ${props.flow.response_mime || ''}`.toLowerCase()
  return protocol.includes('grpc') || mime.includes('grpc')
})

const grpcDetected = computed(() => {
  if (!result.value) return false
  return (
    format.value === 'grpc' ||
    flowIsGrpc.value ||
    (result.value.detected || '').toLowerCase().includes('grpc') ||
    (result.value.format || '').toLowerCase().includes('grpc')
  )
})

const metaMessages = computed<unknown[]>(() => {
  const meta = result.value?.meta
  if (!meta) return []
  for (const key of ['messages', 'decoded_messages', 'grpc_messages', 'items']) {
    const value = meta[key]
    if (Array.isArray(value)) return value
  }
  const tree = result.value?.tree as Record<string, unknown> | null
  if (tree && typeof tree === 'object' && Array.isArray(tree.messages)) {
    return tree.messages
  }
  return []
})

const grpcMessageCount = computed(() => {
  if (!result.value) return 0
  const meta = result.value.meta || {}
  for (const key of ['message_count', 'messages_count', 'count']) {
    const value = meta[key]
    if (typeof value === 'number' && Number.isFinite(value)) return value
  }
  if (metaMessages.value.length) return metaMessages.value.length
  const match = /(\d+)\s+messages?/i.exec(result.value.summary || '')
  return match ? Number(match[1]) : 0
})

const metaEntries = computed(() => {
  const meta = result.value?.meta
  if (!meta) return []
  return Object.entries(meta).map(([key, value]) => {
    let display = ''
    if (value == null) display = '—'
    else if (typeof value === 'object') {
      const serialized = jsonStringify(value)
      display = serialized.length > 160 ? `${serialized.slice(0, 160)}…` : serialized
    } else display = String(value)
    return { key, display, full: jsonStringify(value) }
  })
})

const hasTree = computed(() => {
  if (!result.value) return false
  const tree = result.value.tree
  if (tree === null || tree === undefined) return false
  if (typeof tree === 'object') return Object.keys(tree as object).length > 0
  return true
})

function reset(): void {
  result.value = null
  error.value = ''
}

watch([direction, format], () => {
  reset()
  // Keep the view in sync: switching direction or format re-runs the decoder
  // instead of leaving an empty panel behind.
  void runDecode()
})

// A background detail refresh can hand over a new flow object for the same
// panel instance; re-decode when the flow itself changes.
watch(
  () => props.flow.id,
  () => {
    reset()
    void runDecode()
  },
)

async function runDecode(): Promise<void> {
  controller?.abort()
  controller = new AbortController()
  loading.value = true
  error.value = ''
  result.value = null
  try {
    result.value = await api.decodeFlow(
      props.flow.id,
      direction.value,
      format.value,
      {},
      controller.signal,
    )
  } catch (decodeError) {
    if (!isAbortError(decodeError)) {
      error.value =
        decodeError instanceof Error ? decodeError.message : String(decodeError)
      toastError(decodeError, 'Decode failed')
    }
  } finally {
    loading.value = false
    controller = null
  }
}

async function copyResult(): Promise<void> {
  if (!result.value) return
  try {
    await copyText(jsonStringify(result.value))
    copied.value = true
    window.setTimeout(() => {
      copied.value = false
    }, 1600)
  } catch (copyError) {
    toastError(copyError, 'Could not copy decode result')
  }
}

onMounted(async () => {
  decoderController = new AbortController()
  try {
    const response = await api.getDecoders(decoderController.signal)
    decoders.value = response.items || []
  } catch (decoderError) {
    if (!isAbortError(decoderError)) decoders.value = []
  }
  // Opening the Decode tab is already an explicit request to inspect the wire
  // format, so run auto-detect instead of making the user click Decode first.
  void runDecode()
})

onBeforeUnmount(() => {
  controller?.abort()
  decoderController?.abort()
})
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <div class="shrink-0 border-b border-ink-700/60 bg-ink-925/35 px-3 py-2">
      <div class="flex flex-wrap items-end gap-2">
        <div class="min-w-[130px] flex-1">
          <span class="field-label mb-1 text-[9px]">Direction</span>
          <div class="segmented w-full">
            <button
              type="button"
              class="flex-1 justify-center"
              :class="{ 'is-active': direction === 'request' }"
              @click="direction = 'request'"
            >Request</button>
            <button
              type="button"
              class="flex-1 justify-center"
              :class="{ 'is-active': direction === 'response' }"
              @click="direction = 'response'"
            >Response</button>
          </div>
        </div>
        <div class="min-w-[160px] flex-[1.3]">
          <label class="field-label mb-1 text-[9px]" for="decode-format">Format</label>
          <select id="decode-format" v-model="format" class="field h-7">
            <option v-for="item in formats" :key="item.id" :value="item.id">
              {{ item.name }}
            </option>
          </select>
        </div>
        <button
          class="btn btn-primary btn-sm h-7"
          type="button"
          :disabled="loading"
          @click="runDecode"
        >
          <AppIcon
            :name="loading ? 'refresh' : 'wand'"
            :size="13"
            :class="loading ? 'animate-soft-spin' : ''"
          />
          {{ loading ? 'Decoding…' : 'Decode' }}
        </button>
      </div>
      <p class="mt-1.5 flex items-center gap-1 text-[9px] text-mist-700">
        <AppIcon name="info" :size="9" />
        Runs on the backend against the captured {{ direction }} body.
      </p>
    </div>

    <div class="app-scroll min-h-0 flex-1 px-3 py-3">
      <div
        v-if="error"
        class="rounded-md border border-rose-500/20 bg-rose-500/[0.05] p-3"
      >
        <div class="flex items-center gap-1.5 text-[10.5px] font-semibold text-rose-300">
          <AppIcon name="alert-triangle" :size="12" />
          Decoder error
        </div>
        <p class="mt-1 break-words font-mono text-[10px] leading-relaxed text-rose-300/65">
          {{ error }}
        </p>
        <button class="btn btn-sm mt-2" type="button" @click="runDecode">Retry</button>
      </div>

      <div
        v-else-if="!result && !loading"
        class="flex min-h-[280px] flex-col items-center justify-center px-4 text-center"
      >
        <div class="relative flex h-11 w-11 items-center justify-center rounded-xl border border-ink-700 bg-ink-900/45">
          <AppIcon name="wand" :size="19" class="text-mist-600" />
          <span class="absolute -right-1 -top-1 h-2 w-2 rounded-full border border-ink-900 bg-accent-500/70" />
        </div>
        <h3 class="mt-3 text-[12px] font-semibold text-mist-400">Inspect the wire format</h3>
        <p class="mt-1 max-w-[280px] text-[10.5px] leading-relaxed text-mist-600">
          Pick a decoder to inspect protobuf fields, gRPC messages, JSON, forms,
          and other payloads. Auto-detect works for known content types.
        </p>
        <button class="btn btn-sm mt-3" type="button" @click="runDecode">
          <AppIcon name="wand" :size="12" />
          Run auto-detect
        </button>
      </div>

      <div v-else-if="loading && !result" class="space-y-2.5">
        <div class="skeleton h-14 rounded-md" />
        <div class="skeleton h-7 w-2/3 rounded" />
        <div class="skeleton h-36 rounded-md" />
      </div>

      <template v-else-if="result">
        <div class="flex items-start justify-between gap-2">
          <div class="min-w-0">
            <div class="flex flex-wrap items-center gap-1.5">
              <span
                class="badge border-accent-500/25 bg-accent-500/[0.07] text-accent-300"
              >{{ result.detected || result.format || format }}</span>
              <span
                class="badge border-ink-700 bg-ink-800/50 text-mist-500"
              >{{ direction }}</span>
              <span v-if="grpcDetected" class="badge border-teal-500/25 bg-teal-500/[0.06] text-teal-300">
                {{ grpcMessageCount }} message{{ grpcMessageCount === 1 ? '' : 's' }}
              </span>
            </div>
            <p class="mt-1.5 text-[11px] leading-relaxed text-mist-400">
              {{ result.summary || 'Decoded successfully.' }}
            </p>
          </div>
          <button
            class="icon-btn icon-btn-sm shrink-0"
            type="button"
            :title="copied ? 'Copied' : 'Copy result JSON'"
            aria-label="Copy decode result"
            @click="copyResult"
          >
            <AppIcon :name="copied ? 'check' : 'copy'" :size="12" :class="copied ? 'text-emerald-400' : ''" />
          </button>
        </div>

        <div
          v-if="metaEntries.length"
          class="mt-3 overflow-hidden rounded-md border border-ink-700/70 bg-ink-950/25"
        >
          <div class="flex items-center gap-1.5 border-b border-ink-700/50 px-2.5 py-1.5">
            <AppIcon name="list" :size="10" class="text-mist-600" />
            <span class="text-[9px] font-semibold uppercase tracking-[0.08em] text-mist-600">Meta</span>
          </div>
          <div class="divide-y divide-ink-700/30">
            <div
              v-for="entry in metaEntries"
              :key="entry.key"
              class="grid grid-cols-[minmax(90px,0.8fr)_minmax(0,1.7fr)] gap-2 px-2.5 py-1.5"
              :title="`${entry.key}: ${entry.full}`"
            >
              <span class="truncate font-mono text-[9.5px] text-mist-600">{{ entry.key }}</span>
              <span class="break-all font-mono text-[9.5px] text-mist-400">{{ entry.display }}</span>
            </div>
          </div>
        </div>

        <div v-if="grpcDetected" class="mt-3">
          <div class="mb-1.5 flex items-center justify-between">
            <h4 class="section-title">Decoded messages</h4>
            <span class="mono text-[9px] text-mist-700">{{ grpcMessageCount }} total</span>
          </div>
          <div v-if="metaMessages.length" class="space-y-1.5">
            <details
              v-for="(message, index) in metaMessages"
              :key="index"
              class="group overflow-hidden rounded-md border border-ink-700/70 bg-ink-950/25"
              :open="index < 2"
            >
              <summary
                class="flex cursor-pointer list-none items-center gap-1.5 px-2.5 py-1.5 text-[10px] font-medium text-mist-400 marker:hidden hover:bg-[var(--ui-hover-soft)]"
              >
                <AppIcon name="chevron-right" :size="10" class="text-mist-600 transition-transform group-open:rotate-90" />
                Message {{ index + 1 }}
                <span class="ml-auto font-mono text-[9px] text-mist-700">
                  {{ typeof message === 'object' ? 'decoded' : typeof message }}
                </span>
              </summary>
              <div class="border-t border-ink-700/40 p-2.5">
                <DecodeTree :value="message" is-root />
              </div>
            </details>
          </div>
          <div
            v-else
            class="rounded-md border border-ink-700/60 bg-ink-950/25 px-2.5 py-2 text-[10px] leading-relaxed text-mist-600"
          >
            {{ grpcMessageCount }} gRPC message{{ grpcMessageCount === 1 ? '' : 's' }} detected.
            Message payloads are included in the decode tree below when available.
          </div>
        </div>

        <div v-if="hasTree" class="mt-3 overflow-hidden rounded-md border border-ink-700/70 bg-[var(--ui-sunken)]">
          <div class="flex items-center justify-between border-b border-ink-700/50 px-2.5 py-1.5">
            <div class="flex items-center gap-1.5">
              <AppIcon name="code" :size="10" class="text-mist-600" />
              <span class="text-[9px] font-semibold uppercase tracking-[0.08em] text-mist-600">Decode tree</span>
            </div>
            <span class="font-mono text-[8.5px] text-mist-700">click to expand</span>
          </div>
          <div class="app-scroll max-h-[360px] overflow-auto p-2.5">
            <DecodeTree :value="result.tree" is-root />
          </div>
        </div>

        <div v-if="result.text" class="mt-3 overflow-hidden rounded-md border border-ink-700/70 bg-[var(--ui-sunken)]">
          <div class="flex items-center justify-between border-b border-ink-700/50 px-2.5 py-1.5">
            <div class="flex items-center gap-1.5">
              <AppIcon name="terminal" :size="10" class="text-mist-600" />
              <span class="text-[9px] font-semibold uppercase tracking-[0.08em] text-mist-600">Decoded text</span>
            </div>
            <span class="font-mono text-[8.5px] text-mist-700">{{ formatBytes(result.text.length) }}</span>
          </div>
          <pre class="app-scroll max-h-[300px] overflow-auto whitespace-pre-wrap break-words px-2.5 py-2 font-mono text-[10px] leading-[1.65] text-[var(--ui-code-text-soft)]">{{ result.text }}</pre>
        </div>

        <p
          v-if="!hasTree && !result.text && !metaEntries.length && !grpcDetected"
          class="mt-4 rounded border border-ink-700/60 bg-ink-950/30 px-2.5 py-2 text-[10px] text-mist-600"
        >
          The decoder completed without a renderable payload.
        </p>
      </template>
    </div>
  </div>
</template>
