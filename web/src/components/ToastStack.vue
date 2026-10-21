<script setup lang="ts">
import AppIcon from './AppIcon.vue'
import { useToasts } from '../composables/useToasts'
import type { Toast } from '../types'

const { toasts, dismiss } = useToasts()

const tone: Record<Toast['kind'], { border: string; icon: string; iconColor: string }> = {
  success: {
    border: 'border-emerald-500/25',
    icon: 'check-circle',
    iconColor: 'text-emerald-400',
  },
  error: {
    border: 'border-rose-500/30',
    icon: 'alert-triangle',
    iconColor: 'text-rose-400',
  },
  info: {
    border: 'border-accent-500/25',
    icon: 'info',
    iconColor: 'text-accent-400',
  },
}
</script>

<template>
  <Teleport to="body">
    <div
      class="pointer-events-none fixed bottom-12 right-3 z-[100] flex w-[min(380px,calc(100vw-24px))] flex-col gap-2 sm:right-4"
      aria-live="polite"
      aria-atomic="false"
    >
      <TransitionGroup name="toast" tag="div" class="flex flex-col gap-2">
        <div
          v-for="toast in toasts"
          :key="toast.id"
          class="pointer-events-auto flex items-start gap-2.5 rounded-lg border bg-ink-900/95 p-3 shadow-[0_12px_36px_rgba(0,0,0,0.5)] backdrop-blur"
          :class="tone[toast.kind].border"
          role="status"
        >
          <AppIcon
            :name="tone[toast.kind].icon"
            :size="16"
            class="mt-0.5"
            :class="tone[toast.kind].iconColor"
          />
          <div class="min-w-0 flex-1">
            <p class="text-xs font-semibold text-mist-200">{{ toast.title }}</p>
            <p
              v-if="toast.message"
              class="mt-0.5 break-words text-2xs leading-relaxed text-mist-500"
            >
              {{ toast.message }}
            </p>
          </div>
          <button
            class="icon-btn icon-btn-tiny -mr-1 -mt-1"
            type="button"
            aria-label="Dismiss notification"
            @click="dismiss(toast.id)"
          >
            <AppIcon name="x" :size="12" />
          </button>
        </div>
      </TransitionGroup>
    </div>
  </Teleport>
</template>
