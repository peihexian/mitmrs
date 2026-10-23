<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import AppIcon from './AppIcon.vue'
import type { BodyView } from '../types'
import {
  bodyBytes,
  bodyText,
  copyText,
  downloadBlob,
  formatBytes,
  hexDump,
  looksBinary,
  prettyBody,
} from '../lib/format'
import { toastError } from '../composables/useToasts'

const props = withDefaults(
  defineProps<{
    view: BodyView | null
    direction: 'request' | 'response'
    filenamePrefix?: string
  }>(),
  { filenamePrefix: 'mitmrs' },
)

type ViewMode = 'pretty' | 'raw' | 'hex'

const mode = ref<ViewMode>('pretty')
const copied = ref(false)
let copyTimer: number | null = null

const rawText = computed(() => bodyText(props.view, 4 * 1024 * 1024))
const bytes = computed(() => bodyBytes(props.view))
const binary = computed(() => {
  if (!props.view) return false
  if (bytes.value.length === 0) return false
  return looksBinary(bytes.value)
})
const hasText = computed(() => rawText.value != null && !binary.value)
const hasBody = computed(() => {
  if (!props.view) return false
  return (
    props.view.body_size > 0 ||
    Boolean(props.view.body_text) ||
    Boolean(props.view.body_base64)
  )
})
const formatted = computed(() =>
  prettyBody(rawText.value ?? '', props.view?.content_type ?? null),
)
const displayText = computed(() => {
  if (!hasText.value) return ''
  return mode.value === 'pretty' ? formatted.value.text : (rawText.value ?? '')
})
const hexText = computed(() => hexDump(bytes.value))
const displayValue = computed(() => {
  if (mode.value === 'hex') return hexText.value
  return displayText.value
})
const lineCount = computed(() => displayValue.value.split('\n').length)
const contentLabel = computed(() => {
  if (!props.view) return 'No body'
  return props.view.content_type || 'unknown content type'
})
const downloadExtension = computed(() => {
  const type = (props.view?.content_type || '').toLowerCase()
  if (type.includes('json')) return 'json'
  if (type.includes('html')) return 'html'
  if (type.includes('xml')) return 'xml'
  if (type.includes('javascript')) return 'js'
  if (type.includes('css')) return 'css'
  if (type.includes('form')) return 'txt'
  if (binary.value) return 'bin'
  return 'txt'
})

watch(
  () => props.view,
  () => {
    mode.value = binary.value ? 'hex' : 'pretty'
    copied.value = false
  },
  { immediate: true },
)

async function onCopy(): Promise<void> {
  if (!hasBody.value) return
  try {
    await copyText(displayValue.value)
    copied.value = true
    if (copyTimer != null) window.clearTimeout(copyTimer)
    copyTimer = window.setTimeout(() => {
      copied.value = false
    }, 1600)
  } catch (error) {
    toastError(error, 'Could not copy the body')
  }
}

function onDownload(): void {
  if (!hasBody.value) return
  const blobBytes =
    bytes.value.length > 0
      ? bytes.value
      : new TextEncoder().encode(rawText.value ?? '')
  const blob = new Blob([Uint8Array.from(blobBytes)], {
    type:
      props.view?.content_type || (binary.value ? 'application/octet-stream' : 'text/plain'),
  })
  downloadBlob(
    blob,
    `${props.filenamePrefix}-${props.direction}.${downloadExtension.value}`,
  )
}

onBeforeUnmount(() => {
  if (copyTimer != null) window.clearTimeout(copyTimer)
})

defineExpose({
  copy: onCopy,
  download: onDownload,
})
</script>

<template>
  <section class="flex h-full min-h-0 flex-col">
    <div
      class="flex min-h-10 shrink-0 flex-wrap items-center gap-x-2 gap-y-1.5 border-b border-ink-700/60 bg-ink-925/35 px-3 py-1.5"
    >
      <div class="flex min-w-0 items-center gap-1.5">
        <span
          class="flex h-4 w-4 items-center justify-center rounded"
          :class="
            direction === 'request'
              ? 'bg-sky-500/10 text-sky-400'
              : 'bg-teal-500/10 text-teal-400'
          "
        >
          <AppIcon :name="direction === 'request' ? 'arrow-up' : 'arrow-down'" :size="10" />
        </span>
        <span class="text-[10.5px] font-semibold text-mist-400">
          {{ direction === 'request' ? 'Request body' : 'Response body' }}
        </span>
        <span class="mono text-[9.5px] text-mist-600">{{ formatBytes(view?.body_size) }}</span>
        <span
          class="hidden max-w-[210px] truncate rounded border border-ink-700 bg-ink-950/40 px-1 py-px font-mono text-[9px] text-mist-600 sm:inline"
          :title="contentLabel"
        >{{ contentLabel }}</span>
        <span
          v-if="view?.body_encoding"
          class="rounded border border-sky-500/20 bg-sky-500/[0.06] px-1 py-px font-mono text-[8.5px] text-sky-400"
          :title="`Body encoding: ${view.body_encoding}`"
        >{{ view.body_encoding }}</span>
      </div>

      <div class="ml-auto flex items-center gap-1.5">
        <div class="segmented">
          <button
            type="button"
            :class="{ 'is-active': mode === 'pretty' }"
            :disabled="!hasText"
            :title="hasText ? 'Formatted text' : 'Binary body — use hex view'"
            @click="mode = 'pretty'"
          >Pretty</button>
          <button
            type="button"
            :class="{ 'is-active': mode === 'raw' }"
            :disabled="!hasText"
            title="Raw text"
            @click="mode = 'raw'"
          >Raw</button>
          <button
            type="button"
            :class="{ 'is-active': mode === 'hex' }"
            title="Hex dump"
            @click="mode = 'hex'"
          >Hex</button>
        </div>
        <button
          class="icon-btn icon-btn-sm"
          type="button"
          :disabled="!hasBody"
          :title="copied ? 'Copied' : 'Copy body'"
          aria-label="Copy body"
          @click="onCopy"
        >
          <AppIcon :name="copied ? 'check' : 'copy'" :size="13" :class="copied ? 'text-emerald-400' : ''" />
        </button>
        <button
          class="icon-btn icon-btn-sm"
          type="button"
          :disabled="!hasBody"
          title="Download body"
          aria-label="Download body"
          @click="onDownload"
        >
          <AppIcon name="download" :size="13" />
        </button>
      </div>
    </div>

    <div
      v-if="view?.truncated"
      class="flex shrink-0 items-center gap-1.5 border-b border-amber-500/15 bg-amber-500/[0.045] px-3 py-1 text-[9.5px] text-amber-300/80"
    >
      <AppIcon name="alert-triangle" :size="11" />
      Body truncated by the capture size limit. Download contains the captured prefix.
    </div>

    <div v-if="!hasBody" class="flex flex-1 flex-col items-center justify-center px-6 text-center">
      <div class="flex h-10 w-10 items-center justify-center rounded-xl border border-ink-700 bg-ink-900/40">
        <AppIcon name="file-json" :size="18" class="text-mist-700" />
      </div>
      <p class="mt-2.5 text-[11.5px] font-medium text-mist-500">No body captured</p>
      <p class="mt-0.5 max-w-[260px] text-[10px] leading-relaxed text-mist-700">
        {{ view?.body_size ? 'The body is unavailable for this view.' : 'This message has no payload, or bodies were disabled in capture settings.' }}
      </p>
    </div>

    <div v-else class="app-scroll relative min-h-0 flex-1 overflow-auto bg-[var(--ui-sunken)]">
      <div class="sticky top-0 z-[1] flex h-6 items-center justify-between border-b border-ink-700/40 bg-ink-925/85 px-3 backdrop-blur-sm">
        <span class="font-mono text-[8.5px] uppercase tracking-[0.08em] text-mist-700">
          {{ mode === 'hex' ? 'Hex dump' : mode === 'pretty' ? formatted.language : 'Raw' }}
        </span>
        <span class="font-mono text-[8.5px] tabular-nums text-mist-700">{{ lineCount }} lines</span>
      </div>
      <pre
        v-if="mode !== 'hex'"
        class="mono whitespace-pre-wrap break-words px-3.5 py-3 text-[11px] leading-[1.65] text-[var(--ui-code-text-soft)] selection:bg-accent-500/25"
      >{{ displayText || '(empty body)' }}</pre>
      <pre
        v-else
        class="mono whitespace-pre px-3 py-2.5 text-[10px] leading-[1.6] text-[var(--ui-code-text-dim)]"
      >{{ hexText || '(no bytes)' }}</pre>
    </div>
  </section>
</template>
