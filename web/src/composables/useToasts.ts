import { readonly, ref } from 'vue'
import type { Toast, ToastKind } from '../types'
import { errorMessage } from './useApi'

const items = ref<Toast[]>([])
const timers = new Map<number, number>()
let sequence = 0

function remove(id: number): void {
  items.value = items.value.filter((toast) => toast.id !== id)
  const timer = timers.get(id)
  if (timer != null) {
    window.clearTimeout(timer)
    timers.delete(id)
  }
}

function push(
  kind: ToastKind,
  title: string,
  message?: string,
  detail?: string,
): number {
  const id = ++sequence
  items.value = [...items.value.slice(-4), { id, kind, title, message, detail }]
  const ttl = kind === 'error' ? 8000 : 4500
  timers.set(id, window.setTimeout(() => remove(id), ttl))
  return id
}

export function toastSuccess(title: string, message?: string): number {
  return push('success', title, message)
}

export function toastInfo(title: string, message?: string): number {
  return push('info', title, message)
}

export function toastError(
  titleOrError: unknown,
  fallback = 'Something went wrong',
): number {
  const message =
    typeof titleOrError === 'string'
      ? titleOrError
      : errorMessage(titleOrError)
  return push('error', fallback, message)
}

export function useToasts() {
  return {
    toasts: readonly(items),
    dismiss: remove,
    clear: () => {
      for (const toast of items.value) remove(toast.id)
    },
    success: toastSuccess,
    info: toastInfo,
    error: toastError,
  }
}
