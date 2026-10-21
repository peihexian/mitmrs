<script setup lang="ts">
import { computed } from 'vue'
import AppIcon from './AppIcon.vue'
import type { LiveTransport } from '../composables/useLive'

const props = defineProps<{
  visibleCount: number
  total: number
  loading: boolean
  live: boolean
  transport: LiveTransport
  activeFilters: number
}>()

const emit = defineEmits<{
  refresh: []
  'toggle-live': []
  clear: []
  'open-filters': []
}>()

const liveLabel = computed(() => {
  if (!props.live) return 'Paused'
  if (props.transport === 'sse') return 'Live'
  if (props.transport === 'polling') return 'Polling'
  return 'Connecting'
})

const liveTone = computed(() => {
  if (!props.live) return 'text-mist-600'
  if (props.transport === 'sse') return 'text-emerald-400'
  if (props.transport === 'polling') return 'text-amber-400'
  return 'text-mist-500'
})
</script>

<template>
  <div
    class="flex h-11 shrink-0 items-center gap-2 border-b border-ink-700/70 bg-ink-925/50 px-2.5 sm:px-3"
  >
    <button
      class="icon-btn icon-btn-sm relative lg:hidden"
      type="button"
      aria-label="Open filters"
      title="Filters"
      @click="emit('open-filters')"
    >
      <AppIcon name="filter" :size="14" />
      <span
        v-if="activeFilters"
        class="absolute right-1 top-1 h-1.5 w-1.5 rounded-full bg-accent-400"
      />
    </button>

    <div class="flex min-w-0 items-center gap-2">
      <AppIcon name="activity" :size="14" class="text-accent-500/80" />
      <h1 class="text-[12px] font-semibold tracking-tight text-mist-300">Traffic</h1>
      <span class="mono text-[10px] tabular-nums text-mist-600">
        {{ total.toLocaleString() }}
      </span>
      <span
        v-if="activeFilters"
        class="hidden items-center gap-1 rounded border border-ink-700 bg-ink-900/60 px-1.5 py-px text-[9px] text-mist-500 sm:inline-flex"
      >
        {{ activeFilters }} filter{{ activeFilters === 1 ? '' : 's' }}
        <button
          class="text-mist-600 hover:text-mist-300"
          type="button"
          title="Clear filters"
          @click="emit('clear')"
        >
          <AppIcon name="x" :size="10" />
        </button>
      </span>
    </div>

    <div class="ml-auto flex items-center gap-1.5">
      <span class="hidden items-center gap-1.5 md:flex" :class="liveTone">
        <span
          class="h-1.5 w-1.5 rounded-full"
          :class="[
            live && transport === 'sse'
              ? 'bg-emerald-400 animate-pulse-dot'
              : live && transport === 'polling'
                ? 'bg-amber-400'
                : 'bg-mist-700',
          ]"
        />
        <span class="text-[9.5px] font-semibold uppercase tracking-[0.07em]">
          {{ liveLabel }}
        </span>
      </span>

      <button
        class="flex h-7 items-center gap-1.5 rounded-md border px-1.5 text-[10px] font-medium transition"
        :class="
          live
            ? 'border-accent-500/25 bg-accent-500/[0.07] text-accent-300'
            : 'border-ink-700 bg-ink-900/50 text-mist-600'
        "
        type="button"
        role="switch"
        :aria-checked="live"
        title="Toggle live updates"
        @click="emit('toggle-live')"
      >
        <span
          class="relative h-3.5 w-6 rounded-full transition"
          :class="live ? 'bg-accent-500/70' : 'bg-ink-650'"
        >
          <span
            class="absolute top-0.5 h-2.5 w-2.5 rounded-full bg-white shadow-sm transition-all"
            :class="live ? 'left-[13px]' : 'left-0.5'"
          />
        </span>
        <span class="hidden sm:inline">Live</span>
      </button>

      <span class="mx-0.5 hidden h-4 w-px bg-ink-700 sm:block" />

      <button
        class="icon-btn icon-btn-sm"
        type="button"
        title="Refresh flows"
        aria-label="Refresh flows"
        :disabled="loading"
        @click="emit('refresh')"
      >
        <AppIcon name="refresh" :size="14" :class="loading ? 'animate-soft-spin' : ''" />
      </button>
      <span
        class="hidden font-mono text-[9px] tabular-nums text-mist-700 sm:inline"
        title="Rows loaded / matching total"
      >{{ visibleCount }} / {{ total }}</span>
    </div>
  </div>
</template>
