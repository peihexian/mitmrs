<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import AppIcon from './AppIcon.vue'
import Modal from './Modal.vue'
import CopyBlock from './CopyBlock.vue'
import type { CaInfo } from '../types'
import { api, errorMessage, isAbortError } from '../composables/useApi'
import { copyText, formatCalendarDate } from '../lib/format'
import { toastError, toastSuccess } from '../composables/useToasts'

const emit = defineEmits<{ close: [] }>()

const ca = ref<CaInfo | null>(null)
const loading = ref(true)
const error = ref('')
const downloading = ref(false)
const copiedField = ref('')
let controller: AbortController | null = null

const certutilCommand = computed(
  () =>
    '# Run PowerShell as Administrator\ncertutil -addstore -f "ROOT" "$env:USERPROFILE\\Downloads\\mitmrs-ca.crt"',
)
const powershellCommand = computed(
  () =>
    '# Run PowerShell as Administrator\nImport-Certificate -FilePath "$env:USERPROFILE\\Downloads\\mitmrs-ca.crt" -CertStoreLocation Cert:\\LocalMachine\\Root',
)
const validity = computed(() => {
  if (!ca.value) return '—'
  return `${formatCalendarDate(ca.value.not_before)} → ${formatCalendarDate(ca.value.not_after)}`
})
async function loadCa(): Promise<void> {
  controller?.abort()
  controller = new AbortController()
  loading.value = true
  error.value = ''
  try {
    ca.value = await api.getCa(controller.signal)
  } catch (loadError) {
    if (!isAbortError(loadError)) {
      error.value = errorMessage(loadError)
    }
  } finally {
    loading.value = false
  }
}

async function downloadCa(): Promise<void> {
  downloading.value = true
  try {
    const filename = await api.downloadCa()
    toastSuccess('CA certificate downloaded', filename)
  } catch (downloadError) {
    toastError(downloadError, 'CA download failed')
  } finally {
    downloading.value = false
  }
}

async function copyField(field: string, value: string | undefined): Promise<void> {
  if (!value) return
  try {
    await copyText(value)
    copiedField.value = field
    window.setTimeout(() => {
      copiedField.value = ''
    }, 1600)
  } catch (copyError) {
    toastError(copyError, 'Could not copy')
  }
}

onMounted(loadCa)
onBeforeUnmount(() => controller?.abort())
</script>

<template>
  <Modal
    title="Certification authority"
    subtitle="Install the MitmRs root CA to inspect HTTPS traffic."
    size="lg"
    @close="emit('close')"
  >
    <div v-if="loading" class="space-y-2.5">
      <div class="skeleton h-24 rounded-md" />
      <div class="skeleton h-16 rounded-md" />
      <div class="skeleton h-28 rounded-md" />
    </div>

    <div v-else-if="error" class="rounded-lg border border-rose-500/20 bg-rose-500/[0.04] p-4">
      <div class="flex items-center gap-2 text-[11.5px] font-semibold text-rose-300">
        <AppIcon name="alert-triangle" :size="14" />
        Could not load the CA
      </div>
      <p class="mt-1 break-words text-[10px] leading-relaxed text-rose-300/65">{{ error }}</p>
      <button class="btn btn-sm mt-3" type="button" @click="loadCa">
        <AppIcon name="refresh" :size="12" />
        Retry
      </button>
    </div>

    <template v-else-if="ca">
      <div class="overflow-hidden rounded-lg border border-ink-700 bg-ink-950/30">
        <div class="flex items-start gap-3 border-b border-ink-700/60 p-3.5">
          <div class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-accent-500/25 bg-accent-500/[0.05]">
            <AppIcon name="shield-check" :size="19" class="text-accent-400" />
          </div>
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-2">
              <span class="text-[11.5px] font-semibold text-mist-200">MitmRs Root CA</span>
              <span class="badge border-emerald-500/20 bg-emerald-500/[0.05] text-emerald-300/80">Local issuer</span>
            </div>
            <p class="mono mt-1 break-all text-[9.5px] leading-relaxed text-mist-500">{{ ca.subject }}</p>
            <p class="mt-1 flex items-center gap-1 text-[9.5px] text-mist-600">
              <AppIcon name="clock" :size="9" />
              Valid {{ validity }}
            </p>
          </div>
          <button class="btn btn-sm btn-primary shrink-0" type="button" :disabled="downloading" @click="downloadCa">
            <AppIcon :name="downloading ? 'refresh' : 'download'" :size="12" :class="downloading ? 'animate-soft-spin' : ''" />
            <span class="hidden sm:inline">Download .crt</span>
          </button>
        </div>

        <div class="divide-y divide-ink-700/40">
          <div class="grid grid-cols-[90px_minmax(0,1fr)_24px] items-start gap-2 px-3.5 py-2">
            <span class="text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">Fingerprint</span>
            <span class="mono break-all text-[9px] leading-relaxed text-mist-400" :title="ca.fingerprint">{{ ca.fingerprint || 'Not available' }}</span>
            <button class="icon-btn icon-btn-tiny -mt-0.5" type="button" title="Copy fingerprint" aria-label="Copy fingerprint" @click="copyField('fingerprint', ca.fingerprint)">
              <AppIcon :name="copiedField === 'fingerprint' ? 'check' : 'copy'" :size="10" :class="copiedField === 'fingerprint' ? 'text-emerald-400' : ''" />
            </button>
          </div>
          <div class="grid grid-cols-[90px_minmax(0,1fr)_24px] items-start gap-2 px-3.5 py-2">
            <span class="text-[8.5px] font-semibold uppercase tracking-wider text-mist-700">On disk</span>
            <span class="mono break-all text-[9px] leading-relaxed text-mist-500">{{ ca.path || 'Managed by backend' }}</span>
            <button class="icon-btn icon-btn-tiny -mt-0.5" type="button" title="Copy certificate path" aria-label="Copy certificate path" :disabled="!ca.path" @click="copyField('path', ca.path)">
              <AppIcon :name="copiedField === 'path' ? 'check' : 'copy'" :size="10" :class="copiedField === 'path' ? 'text-emerald-400' : ''" />
            </button>
          </div>
        </div>
      </div>

      <div class="mt-4">
        <h3 class="section-title">Windows trust setup</h3>
        <ol class="mt-2 space-y-2.5">
          <li class="flex gap-2.5">
            <span class="flex h-4 w-4 shrink-0 items-center justify-center rounded-full border border-ink-650 bg-ink-850 font-mono text-[8px] text-mist-500">1</span>
            <div class="min-w-0">
              <p class="text-[10.5px] font-medium text-mist-300">Download the certificate</p>
              <p class="mt-0.5 text-[9.5px] leading-relaxed text-mist-600">
                Save <span class="mono text-mist-500">mitmrs-ca.crt</span> to your
                Downloads folder.
              </p>
            </div>
          </li>
          <li class="flex gap-2.5">
            <span class="flex h-4 w-4 shrink-0 items-center justify-center rounded-full border border-ink-650 bg-ink-850 font-mono text-[8px] text-mist-500">2</span>
            <div class="min-w-0 flex-1">
              <p class="text-[10.5px] font-medium text-mist-300">Install into Trusted Root</p>
              <p class="mt-1 text-[9.5px] leading-relaxed text-mist-600">
                Double-click the .crt → Install Certificate → Local Machine →
                Place all certificates in <span class="text-mist-500">Trusted Root Certification Authorities</span>.
                Or run one of these as Administrator:
              </p>
              <div class="mt-2 space-y-1.5">
                <CopyBlock :code="certutilCommand" label="PowerShell (Administrator) · certutil" language="powershell" max-height="90px" />
                <CopyBlock :code="powershellCommand" label="PowerShell (Administrator) · Import-Certificate" language="powershell" max-height="90px" />
              </div>
            </div>
          </li>
          <li class="flex gap-2.5">
            <span class="flex h-4 w-4 shrink-0 items-center justify-center rounded-full border border-ink-650 bg-ink-850 font-mono text-[8px] text-mist-500">3</span>
            <div class="min-w-0">
              <p class="text-[10.5px] font-medium text-mist-300">Restart the target app</p>
              <p class="mt-0.5 text-[9.5px] leading-relaxed text-mist-600">
                Browsers and applications cache TLS roots. Fully quit and
                reopen them after installing the CA. Some apps with certificate
                pinning must be configured separately.
              </p>
            </div>
          </li>
        </ol>
      </div>

      <div class="mt-4 rounded-md border border-amber-500/20 bg-amber-500/[0.045] p-3">
        <div class="flex items-center gap-1.5 text-[10px] font-semibold text-amber-300">
          <AppIcon name="alert-triangle" :size="12" />
          Security warning
        </div>
        <p class="mt-1 text-[9.5px] leading-relaxed text-amber-200/55">
          A trusted root CA can decrypt and impersonate any TLS site for any
          user on this machine. Install it only on a dedicated development
          device, and remove it when you finish testing. Never share the private
          key or install this certificate on production systems.
        </p>
        <p class="mt-1.5 font-mono text-[8.5px] text-amber-200/40">
          Remove later: certutil -delstore "ROOT" &lt;serial-or-thumbprint&gt;
        </p>
      </div>
    </template>

    <template #footer>
      <button class="btn btn-sm" type="button" @click="emit('close')">Close</button>
      <button class="btn btn-sm btn-primary" type="button" :disabled="downloading" @click="downloadCa">
        <AppIcon :name="downloading ? 'refresh' : 'download'" :size="12" :class="downloading ? 'animate-soft-spin' : ''" />
        Download certificate
      </button>
    </template>
  </Modal>
</template>
