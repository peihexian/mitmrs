import { onBeforeUnmount, ref } from 'vue'
import type { Ref } from 'vue'

export interface PanelResizeOptions {
  /** localStorage key used to persist the panel width. */
  storageKey: string
  /** Smallest allowed width in CSS pixels. */
  min: number
  /** Absolute upper bound; the live viewport limit is layered on top of it. */
  hardMax: number
  /** Computed default width, used on first visit and after a reset. */
  initial: () => number
  /** Live upper bound, usually derived from the viewport and siblings. */
  max: () => number
  /** `1` when dragging right widens the panel, `-1` when dragging left widens it. */
  direction: 1 | -1
}

export interface PanelResize {
  width: Ref<number>
  dragging: Ref<boolean>
  min: number
  start: (event: PointerEvent) => void
  reset: () => void
  onKeydown: (event: KeyboardEvent) => void
  /** Persist the current width without an active interaction. */
  commit: () => void
}

/**
 * Tracks a horizontally resizable pane: pointer dragging on a splitter,
 * keyboard nudging, double-click reset, and `localStorage` persistence.
 */
export function usePanelResize(options: PanelResizeOptions): PanelResize {
  const width = ref(readWidth())
  const dragging = ref(false)
  let originX = 0
  let originWidth = 0
  let activePointer: number | null = null

  function limits(): { min: number; max: number } {
    const upper = Math.min(options.hardMax, options.max())
    return { min: options.min, max: Math.max(options.min, upper) }
  }

  function clamp(value: number): number {
    const { min, max } = limits()
    return Math.min(Math.max(Math.round(value), min), max)
  }

  function readWidth(): number {
    try {
      const stored = Number.parseFloat(
        window.localStorage.getItem(options.storageKey) ?? '',
      )
      if (Number.isFinite(stored) && stored > 0) {
        return Math.min(Math.max(Math.round(stored), options.min), options.hardMax)
      }
    } catch {
      /* Fall through to the computed default. */
    }
    const fallback = options.initial()
    return Math.min(Math.max(Math.round(fallback), options.min), options.hardMax)
  }

  function persist(value: number): void {
    try {
      window.localStorage.setItem(options.storageKey, String(value))
    } catch {
      /* Persistence is best-effort. */
    }
  }

  function onPointerMove(event: PointerEvent): void {
    if (!dragging.value) return
    const delta = (event.clientX - originX) * options.direction
    width.value = clamp(originWidth + delta)
  }

  function stopDrag(): void {
    window.removeEventListener('pointermove', onPointerMove)
    window.removeEventListener('pointerup', stopDrag)
    window.removeEventListener('pointercancel', stopDrag)
    document.documentElement.classList.remove('is-resizing-layout')
    if (activePointer != null) persist(width.value)
    activePointer = null
    dragging.value = false
  }

  function start(event: PointerEvent): void {
    if (event.pointerType === 'mouse' && event.button !== 0) return
    event.preventDefault()
    stopDrag()
    originX = event.clientX
    originWidth = width.value
    activePointer = event.pointerId
    dragging.value = true
    try {
      const target = event.currentTarget as Element | null
      target?.setPointerCapture?.(event.pointerId)
    } catch {
      /* Pointer capture is a nicety, window listeners still work. */
    }
    window.addEventListener('pointermove', onPointerMove)
    window.addEventListener('pointerup', stopDrag)
    window.addEventListener('pointercancel', stopDrag)
    document.documentElement.classList.add('is-resizing-layout')
  }

  function reset(): void {
    width.value = clamp(options.initial())
    persist(width.value)
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === 'Home') {
      event.preventDefault()
      reset()
      return
    }
    const step = event.shiftKey ? 48 : 16
    let delta = 0
    if (event.key === 'ArrowLeft') delta = -step * options.direction
    else if (event.key === 'ArrowRight') delta = step * options.direction
    if (delta === 0) return
    event.preventDefault()
    width.value = clamp(width.value + delta)
    persist(width.value)
  }

  function settle(): void {
    const next = clamp(width.value)
    if (next !== width.value) width.value = next
  }

  function commit(): void {
    settle()
    persist(width.value)
  }

  onBeforeUnmount(stopDrag)

  return { width, dragging, min: options.min, start, reset, onKeydown, commit }
}
