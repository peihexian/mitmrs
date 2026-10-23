<script setup lang="ts">
import { computed, ref } from 'vue'
import AppIcon from './AppIcon.vue'

defineOptions({ name: 'DecodeTree' })

const props = withDefaults(
  defineProps<{
    value: unknown
    label?: string | number | null
    depth?: number
    isRoot?: boolean
  }>(),
  { label: null, depth: 0, isRoot: false },
)

const initialOpen = computed(() => props.isRoot || props.depth < 2)
const open = ref(initialOpen.value)

const kind = computed<'array' | 'object' | 'string' | 'number' | 'boolean' | 'null' | 'unknown'>(() => {
  if (props.value === null) return 'null'
  if (Array.isArray(props.value)) return 'array'
  switch (typeof props.value) {
    case 'object':
      return 'object'
    case 'string':
      return 'string'
    case 'number':
    case 'bigint':
      return 'number'
    case 'boolean':
      return 'boolean'
    default:
      return 'unknown'
  }
})

const complex = computed(() => kind.value === 'array' || kind.value === 'object')
const entries = computed<Array<[string, unknown]>>(() => {
  if (kind.value === 'array') {
    return (props.value as unknown[]).map((item, index) => [String(index), item])
  }
  if (kind.value === 'object') {
    return Object.entries(props.value as Record<string, unknown>)
  }
  return []
})
const childCount = computed(() => entries.value.length)

const primitiveText = computed(() => {
  if (kind.value === 'string') return JSON.stringify(props.value)
  if (kind.value === 'null') return 'null'
  if (kind.value === 'unknown') return String(props.value)
  return String(props.value)
})
const displayLabel = computed(() => {
  if (props.label == null) return ''
  return Array.isArray(props.label) ? String(props.label) : String(props.label)
})
</script>

<template>
  <div class="decode-tree-node" :class="{ 'is-root-node': isRoot }">
    <div
      class="decode-tree-line group"
      :class="{ 'cursor-pointer': complex }"
      @click="complex && (open = !open)"
    >
      <span class="flex h-5 w-4 shrink-0 items-center justify-center">
        <AppIcon
          v-if="complex"
          name="chevron-right"
          :size="11"
          class="text-mist-700 transition-transform duration-150 group-hover:text-mist-400"
          :class="{ 'rotate-90': open }"
        />
        <span v-else class="h-1 w-1 rounded-full bg-ink-600" />
      </span>

      <span v-if="displayLabel" class="decode-key" :title="displayLabel">
        {{ displayLabel }}
      </span>
      <span v-if="displayLabel" class="mr-1 text-mist-700">:</span>

      <template v-if="complex">
        <span class="type-punctuation">{{ kind === 'array' ? '[' : '{' }}</span>
        <span v-if="!open" class="type-summary">
          {{ childCount }} {{ kind === 'array' ? 'items' : 'fields' }}
        </span>
        <span class="type-punctuation">{{ kind === 'array' ? ']' : '}' }}</span>
      </template>
      <template v-else>
        <span class="decode-value" :class="`kind-${kind}`" :title="primitiveText">
          {{ primitiveText }}
        </span>
      </template>
    </div>

    <div v-if="complex && open" class="decode-tree-children">
      <DecodeTree
        v-for="[key, child] in entries"
        :key="key"
        :value="child"
        :label="key"
        :depth="depth + 1"
      />
      <div v-if="entries.length === 0" class="decode-empty">
        {{ kind === 'array' ? '(empty array)' : '(empty object)' }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.decode-tree-node {
  font-family: var(--font-mono);
  font-size: 10.5px;
  line-height: 1.6;
}

.decode-tree-line {
  display: flex;
  min-height: 22px;
  align-items: flex-start;
  border-radius: 3px;
  padding: 1px 4px 1px 0;
  transition: background-color 100ms ease;
}

.decode-tree-line:hover {
  background: var(--ui-tree-hover);
}

.is-root-node > .decode-tree-line {
  padding-left: 0;
}

.decode-tree-children {
  margin-left: 11px;
  border-left: 1px solid var(--ui-tree-line);
  padding-left: 7px;
}

.decode-key {
  max-width: 42%;
  flex-shrink: 0;
  overflow: hidden;
  color: var(--ui-tree-key);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.decode-value {
  min-width: 0;
  overflow-wrap: anywhere;
  color: var(--ui-tree-value);
}

.kind-string {
  color: var(--ui-tree-string);
}

.kind-number {
  color: var(--ui-tree-number);
}

.kind-boolean {
  color: var(--ui-tree-boolean);
}

.kind-null {
  color: var(--ui-tree-null);
  font-style: italic;
}

.type-punctuation {
  color: var(--ui-tree-punct);
}

.type-summary {
  margin: 0 4px;
  color: var(--ui-tree-summary);
  font-size: 9.5px;
}

.decode-empty {
  padding-left: 25px;
  color: var(--ui-tree-empty);
  font-size: 9.5px;
  font-style: italic;
}
</style>
