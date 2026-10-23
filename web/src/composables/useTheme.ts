import { ref, watch } from 'vue'

export type ThemeName = 'dark' | 'light'

const STORAGE_KEY = 'mitmrs.theme'
const LIGHT_THEME_COLOR = '#f1f5f9'
const DARK_THEME_COLOR = '#080c12'

function readStoredTheme(): ThemeName | null {
  try {
    const value = window.localStorage.getItem(STORAGE_KEY)
    return value === 'light' || value === 'dark' ? value : null
  } catch {
    return null
  }
}

function prefersLight(): boolean {
  return (
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-color-scheme: light)').matches
  )
}

function resolveInitialTheme(): ThemeName {
  const stored = readStoredTheme()
  if (stored) return stored
  const root = document.documentElement
  if (root.classList.contains('theme-light')) return 'light'
  if (root.classList.contains('theme-dark')) return 'dark'
  return prefersLight() ? 'light' : 'dark'
}

/** Single shared theme ref so every component sees the same value. */
const theme = ref<ThemeName>(resolveInitialTheme())

function applyTheme(next: ThemeName): void {
  const root = document.documentElement
  root.classList.toggle('theme-light', next === 'light')
  root.classList.toggle('theme-dark', next === 'dark')
  root.style.colorScheme = next

  const scheme = document.querySelector<HTMLMetaElement>('meta[name="color-scheme"]')
  if (scheme) scheme.content = next

  const themeColor = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]')
  if (themeColor) {
    themeColor.content = next === 'light' ? LIGHT_THEME_COLOR : DARK_THEME_COLOR
  }
}

applyTheme(theme.value)

watch(theme, (next) => {
  applyTheme(next)
  try {
    window.localStorage.setItem(STORAGE_KEY, next)
  } catch {
    /* Persisting the choice is best-effort. */
  }
})

export function useTheme() {
  function setTheme(next: ThemeName): void {
    theme.value = next
  }

  function toggleTheme(): void {
    theme.value = theme.value === 'light' ? 'dark' : 'light'
  }

  return { theme, setTheme, toggleTheme }
}
