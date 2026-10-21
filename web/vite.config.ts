import { defineConfig, loadEnv } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

// The app is embedded by the Rust backend and served from the server root.
// In development, API calls are proxied to the local MitmRs backend. Override
// the target with MITMRS_BACKEND_URL when the backend listens elsewhere.
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, '.', '')
  const backend = env.MITMRS_BACKEND_URL || 'http://127.0.0.1:8787'

  return {
    base: '/',
    plugins: [vue(), tailwindcss()],
    build: {
      outDir: 'dist',
      emptyOutDir: true,
      sourcemap: false,
    },
    server: {
      port: 5173,
      strictPort: true,
      proxy: {
        '/api': { target: backend, changeOrigin: true },
        '/mcp': { target: backend, changeOrigin: true },
      },
    },
    preview: {
      port: 4173,
      proxy: {
        '/api': { target: backend, changeOrigin: true },
        '/mcp': { target: backend, changeOrigin: true },
      },
    },
  }
})
