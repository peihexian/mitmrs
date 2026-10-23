<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import AppIcon from './AppIcon.vue'

const props = withDefaults(
  defineProps<{
    title: string
    subtitle?: string
    size?: 'sm' | 'md' | 'lg' | 'xl'
    closeOnBackdrop?: boolean
  }>(),
  { subtitle: '', size: 'md', closeOnBackdrop: true },
)

const emit = defineEmits<{ close: [] }>()

const panel = ref<HTMLElement | null>(null)
let previousFocus: HTMLElement | null = null
let previousOverflow = ''

const sizeClass = computed(
  () =>
    ({
      sm: 'max-w-[460px]',
      md: 'max-w-[580px]',
      lg: 'max-w-[780px]',
      xl: 'max-w-[980px]',
    })[props.size],
)

function onKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    event.preventDefault()
    emit('close')
  }
  if (event.key !== 'Tab' || !panel.value) return
  const focusable = Array.from(
    panel.value.querySelectorAll<HTMLElement>(
      'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
    ),
  ).filter((element) => element.offsetParent !== null)
  if (focusable.length === 0) return
  const first = focusable[0]
  const last = focusable[focusable.length - 1]
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault()
    last.focus()
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault()
    first.focus()
  }
}

function onBackdrop(): void {
  if (props.closeOnBackdrop) emit('close')
}

onMounted(() => {
  previousFocus = document.activeElement as HTMLElement | null
  previousOverflow = document.body.style.overflow
  document.body.style.overflow = 'hidden'
  window.addEventListener('keydown', onKeydown)
  void nextTick(() => panel.value?.focus())
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown)
  document.body.style.overflow = previousOverflow
  previousFocus?.focus?.()
})
</script>

<template>
  <Teleport to="body">
    <div
      class="fixed inset-0 z-[80] flex items-end justify-center bg-black/65 p-0 backdrop-blur-[3px] sm:items-center sm:p-5"
      @mousedown.self="onBackdrop"
    >
      <div
        ref="panel"
        class="flex max-h-[92dvh] w-full flex-col overflow-hidden rounded-t-xl border border-ink-700 bg-ink-900 shadow-[0_24px_100px_var(--ui-shadow-modal)] outline-none sm:max-h-[86dvh] sm:rounded-xl"
        :class="sizeClass"
        role="dialog"
        aria-modal="true"
        :aria-label="title"
        tabindex="-1"
      >
        <header
          class="flex shrink-0 items-start justify-between gap-4 border-b border-ink-700/80 px-5 py-4"
        >
          <div class="min-w-0">
            <h2 class="text-[15px] font-semibold tracking-tight text-mist-200">
              {{ title }}
            </h2>
            <p v-if="subtitle" class="mt-0.5 text-2xs text-mist-500">
              {{ subtitle }}
            </p>
          </div>
          <button
            class="icon-btn -mr-1 -mt-1"
            type="button"
            title="Close"
            aria-label="Close dialog"
            @click="emit('close')"
          >
            <AppIcon name="x" :size="16" />
          </button>
        </header>

        <div class="app-scroll min-h-0 flex-1 px-5 py-4">
          <slot />
        </div>

        <footer
          v-if="$slots.footer"
          class="flex shrink-0 flex-wrap items-center justify-end gap-2 border-t border-ink-700/80 bg-ink-925/60 px-5 py-3"
        >
          <slot name="footer" />
        </footer>
      </div>
    </div>
  </Teleport>
</template>
