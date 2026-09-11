<script setup lang="ts">
import type { SuiteCatalogItem, SuiteInstanceSummary } from '@/api/interface/suites'
import { VustTag } from '@/components/ui'
import AppIcon from '@/components/icons/AppIcon.vue'

defineProps<{
  suite: SuiteCatalogItem
  instance?: SuiteInstanceSummary
  statusLabel: string
  statusType: 'primary' | 'success' | 'warning' | 'danger' | 'info' | 'default'
}>()

defineEmits<{
  select: []
}>()
</script>

<template>
  <article class="suite-card" data-ui="suite-card" :data-slot="suite.suiteId">
    <button
      type="button"
      class="suite-card__main"
      :aria-label="suite.name"
      @click="$emit('select')"
    >
      <div class="suite-card__head">
        <AppIcon :name="suite.icon" :size="44" :label="suite.name" />
        <div class="suite-card__identity">
          <div class="suite-card__title">{{ suite.name }}</div>
          <div class="suite-card__id">{{ suite.suiteId }}</div>
        </div>
        <VustTag :type="statusType" effect="plain">{{ statusLabel }}</VustTag>
      </div>
      <div class="suite-card__summary">{{ suite.summary }}</div>
      <div v-if="instance?.lastError" class="suite-card__error" role="status">
        {{ instance.lastError }}
      </div>
    </button>
  </article>
</template>

<style scoped>
.suite-card {
  display: grid;
  gap: var(--vdl-space-3);
  min-width: 0;
  padding: var(--vdl-space-4);
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-lg);
  background: var(--vdl-bg-card);
  transition:
    border-color 160ms ease,
    background-color 160ms ease;
}

.suite-card:hover {
  border-color: var(--vdl-border-strong);
  background: var(--vdl-bg-hover);
}

.suite-card__main {
  display: grid;
  gap: var(--vdl-space-4);
  width: 100%;
  min-width: 0;
  padding: 0;
  border: 0;
  color: inherit;
  font: inherit;
  text-align: left;
  background: transparent;
  cursor: pointer;
}

.suite-card__main:focus-visible {
  outline: 2px solid var(--vdl-focus-ring);
  outline-offset: var(--vdl-space-2);
}

.suite-card__head {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-3);
  min-width: 0;
}

.suite-card__identity {
  min-width: 0;
  flex: 1;
}

.suite-card__title {
  overflow: hidden;
  color: var(--vdl-text-primary);
  font: var(--vdl-font-subtitle);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.suite-card__id,
.suite-card__summary,
.suite-card__error {
  overflow: hidden;
  color: var(--vdl-text-muted);
  font: var(--vdl-font-body-sm);
  text-overflow: ellipsis;
}

.suite-card__id {
  white-space: nowrap;
}

.suite-card__summary {
  display: -webkit-box;
  min-height: 2.5em;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}

.suite-card__error {
  color: var(--vdl-danger);
  white-space: nowrap;
}
</style>
