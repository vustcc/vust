<script setup lang="ts">
/**
 * @file SettingsSectionPanel.vue
 * @description 设置应用分区的统一页面骨架。
 */

defineProps<{
  title: string
  page: string
}>()
</script>

<template>
  <div class="settings-section-panel" :data-page="page">
    <div class="settings-section-header" data-slot="header">
      <h2>{{ title }}</h2>
      <div v-if="$slots.actions" class="settings-section-actions" data-slot="actions">
        <slot name="actions" />
      </div>
    </div>
    <div class="settings-section-content" data-slot="content">
      <slot />
    </div>
  </div>
</template>

<style scoped>
.settings-section-panel {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--vdl-bg-panel);
}

.settings-section-header {
  flex-shrink: 0;
  min-height: 48px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3) var(--vdl-space-4);
  border-bottom: 1px solid var(--vdl-border-subtle);
}

.settings-section-header h2 {
  margin: 0;
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-subtitle);
  font-weight: 700;
}

.settings-section-actions {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
}

.settings-section-content {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: var(--vdl-space-4);
}
</style>
