<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import AppIcon from './AppIcon.vue'
import { copyText } from '../lib/format'
import { toastError } from '../composables/useToasts'

const props = withDefaults(
  defineProps<{
    code: string
    label?: string
    language?: string
    maxHeight?: string
  }>(),
  { label: '', language: '', maxHeight: '280px' },
)

const copied = ref(false)
let timer: number | null = null
const displayLanguage = computed(() => props.language || 'text')

async function onCopy(): Promise<void> {
  try {
    await copyText(props.code)
    copied.value = true
    if (timer != null) window.clearTimeout(timer)
    timer = window.setTimeout(() => {
      copied.value = false
    }, 1700)
  } catch (error) {
    toastError(error, 'Could not copy to clipboard')
  }
}

onBeforeUnmount(() => {
  if (timer != null) window.clearTimeout(timer)
})
</script>

<template>
  <div class="copy-code overflow-hidden">
    <div class="flex h-8 items-center gap-2 border-b border-ink-700/60 bg-ink-925/45 px-2.5">
      <span class="min-w-0 truncate text-[9.5px] font-medium text-mist-500">{{ label || displayLanguage }}</span>
      <span class="ml-auto font-mono text-[8px] uppercase tracking-wider text-mist-700">{{ displayLanguage }}</span>
      <button
        class="icon-btn icon-btn-tiny"
        type="button"
        :title="copied ? 'Copied' : 'Copy'"
        aria-label="Copy to clipboard"
        @click="onCopy"
      >
        <AppIcon :name="copied ? 'check' : 'copy'" :size="10" :class="copied ? 'text-emerald-400' : ''" />
      </button>
    </div>
    <pre
      class="mono app-scroll overflow-auto whitespace-pre-wrap break-words px-2.5 py-2 text-[10px] leading-[1.65] text-[var(--ui-code-text)]"
      :style="{ maxHeight }"
    >{{ code }}</pre>
  </div>
</template>
