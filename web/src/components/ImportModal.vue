<script setup lang="ts">
import { computed, ref } from 'vue'
import AppIcon from './AppIcon.vue'
import Modal from './Modal.vue'
import type { ImportResult } from '../types'
import { api, errorMessage } from '../composables/useApi'
import { formatBytes, formatNumber } from '../lib/format'
import { toastError, toastSuccess } from '../composables/useToasts'

const emit = defineEmits<{
  close: []
  imported: [result: ImportResult]
}>()

const input = ref<HTMLInputElement | null>(null)
const payload = ref('')
const filename = ref('')
const fileSize = ref<number | null>(null)
const loading = ref(false)
const dragging = ref(false)
const error = ref('')
const result = ref<ImportResult | null>(null)

const payloadBytes = computed(() => new TextEncoder().encode(payload.value).length)
const canImport = computed(
  () => payload.value.trim().length > 0 && !loading.value,
)
const lineCount = computed(() =>
  payload.value ? payload.value.split('\n').length : 0,
)

function chooseFile(): void {
  input.value?.click()
}

async function readFile(file: File): Promise<void> {
  error.value = ''
  result.value = null
  if (file.size > 64 * 1024 * 1024) {
    error.value = 'This file is larger than 64 MB. Split the export before importing.'
    return
  }
  try {
    payload.value = await file.text()
    filename.value = file.name
    fileSize.value = file.size
  } catch (readError) {
    error.value = readError instanceof Error ? readError.message : 'Could not read the file.'
  }
}

function onFileChange(event: Event): void {
  const inputElement = event.target as HTMLInputElement
  const file = inputElement.files?.[0]
  if (file) void readFile(file)
  inputElement.value = ''
}

function onDrop(event: DragEvent): void {
  dragging.value = false
  const file = event.dataTransfer?.files?.[0]
  if (file) void readFile(file)
}

function clearFile(): void {
  filename.value = ''
  fileSize.value = null
  payload.value = ''
  error.value = ''
  result.value = null
}

async function submit(): Promise<void> {
  if (!canImport.value) return
  error.value = ''
  const trimmed = payload.value.trim()
  try {
    JSON.parse(trimmed)
  } catch {
    error.value = 'The payload is not valid JSON. Paste a native MitmRs JSON export.'
    return
  }

  loading.value = true
  try {
    const response = await api.importFlows(trimmed)
    result.value = response
    emit('imported', response)
    toastSuccess(
      'Import complete',
      `${formatNumber(response.imported)} imported, ${formatNumber(response.skipped)} skipped.`,
    )
  } catch (importError) {
    error.value = errorMessage(importError)
    toastError(importError, 'Import failed')
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <Modal
    title="Import traffic"
    subtitle="Load a native MitmRs JSON export into the current store."
    size="lg"
    @close="emit('close')"
  >
    <div
      v-if="result"
      class="rounded-lg border border-emerald-500/25 bg-emerald-500/[0.04] p-4"
    >
      <div class="flex items-center gap-2">
        <div class="flex h-8 w-8 items-center justify-center rounded-lg bg-emerald-500/10">
          <AppIcon name="check-circle" :size="17" class="text-emerald-400" />
        </div>
        <div>
          <h3 class="text-[12px] font-semibold text-emerald-200">Import complete</h3>
          <p class="mono mt-0.5 text-[10px] text-emerald-300/60">
            {{ formatNumber(result.imported) }} imported · {{ formatNumber(result.skipped) }} skipped
          </p>
        </div>
      </div>
      <p class="mt-3 text-[10.5px] leading-relaxed text-mist-500">
        Imported flows are now part of the store and can be searched, filtered,
        decoded, and read by MCP agents.
      </p>
      <div class="mt-3 grid grid-cols-2 gap-2">
        <div class="rounded-md border border-ink-700/60 bg-ink-950/25 p-2.5">
          <div class="text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">Imported</div>
          <div class="mono mt-1 text-lg text-emerald-300">{{ formatNumber(result.imported) }}</div>
        </div>
        <div class="rounded-md border border-ink-700/60 bg-ink-950/25 p-2.5">
          <div class="text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">Skipped</div>
          <div class="mono mt-1 text-lg text-mist-500">{{ formatNumber(result.skipped) }}</div>
        </div>
      </div>
      <div class="mt-4 flex justify-end gap-2">
        <button class="btn btn-sm" type="button" @click="clearFile">Import another</button>
        <button class="btn btn-sm btn-primary" type="button" @click="emit('close')">Done</button>
      </div>
    </div>

    <template v-else>
      <input
        ref="input"
        type="file"
        accept=".json,application/json"
        class="hidden"
        @change="onFileChange"
      />

      <button
        type="button"
        class="group flex w-full flex-col items-center justify-center rounded-lg border border-dashed px-4 py-5 text-center transition"
        :class="
          dragging
            ? 'border-accent-500/60 bg-accent-500/[0.06]'
            : 'border-ink-650 bg-ink-950/25 hover:border-ink-600 hover:bg-ink-850/30'
        "
        @click="chooseFile"
        @dragover.prevent="dragging = true"
        @dragleave.prevent="dragging = false"
        @drop.prevent="onDrop"
      >
        <div class="flex h-9 w-9 items-center justify-center rounded-lg border border-ink-700 bg-ink-900 text-mist-500 transition group-hover:text-accent-300">
          <AppIcon name="upload" :size="17" />
        </div>
        <span class="mt-2 text-[11px] font-medium text-mist-300">
          Drop a JSON file here, or <span class="text-accent-300">browse</span>
        </span>
        <span class="mt-0.5 text-[9.5px] text-mist-600">
          Native MitmRs export · .json · up to 64 MB
        </span>
      </button>

      <div v-if="filename" class="mt-2 flex items-center gap-2 rounded-md border border-ink-700/60 bg-ink-900/40 px-2.5 py-1.5">
        <AppIcon name="file-json" :size="12" class="text-accent-400/80" />
        <span class="min-w-0 flex-1 truncate font-mono text-[10px] text-mist-400">{{ filename }}</span>
        <span class="font-mono text-[9px] text-mist-700">{{ formatBytes(fileSize) }}</span>
        <button class="icon-btn icon-btn-tiny" type="button" title="Remove file" aria-label="Remove file" @click="clearFile">
          <AppIcon name="x" :size="10" />
        </button>
      </div>

      <div class="mt-3 flex items-center justify-between">
        <label class="field-label mb-0" for="import-payload">
          Or paste JSON
        </label>
        <span class="font-mono text-[8.5px] text-mist-700">
          {{ lineCount }} lines · {{ formatBytes(payloadBytes) }}
        </span>
      </div>
      <textarea
        id="import-payload"
        v-model="payload"
        class="field mono mt-1.5 h-44 resize-y whitespace-pre font-mono text-[10px] leading-relaxed"
        placeholder='{ "version": 1, "flows": [ … ] }'
        spellcheck="false"
        autocomplete="off"
      />

      <div v-if="error" class="mt-2 flex items-start gap-1.5 rounded border border-rose-500/20 bg-rose-500/[0.045] px-2.5 py-2">
        <AppIcon name="alert-triangle" :size="11" class="mt-0.5 shrink-0 text-rose-400" />
        <p class="break-words text-[10px] leading-relaxed text-rose-300/80">{{ error }}</p>
      </div>

      <p class="mt-2 flex items-start gap-1.5 text-[9px] leading-relaxed text-mist-700">
        <AppIcon name="info" :size="10" class="mt-0.5 shrink-0" />
        Import is additive. Existing captured flows are not replaced. The raw
        payload is sent to <span class="mono text-mist-600">/api/import</span>.
      </p>
    </template>

    <template #footer>
      <button class="btn btn-sm" type="button" :disabled="loading" @click="emit('close')">
        {{ result ? 'Close' : 'Cancel' }}
      </button>
      <button
        v-if="!result"
        class="btn btn-sm btn-primary"
        type="button"
        :disabled="!canImport"
        @click="submit"
      >
        <AppIcon :name="loading ? 'refresh' : 'upload'" :size="13" :class="loading ? 'animate-soft-spin' : ''" />
        {{ loading ? 'Importing…' : 'Import flows' }}
      </button>
    </template>
  </Modal>
</template>
