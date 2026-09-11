<script setup lang="ts">
/**
 * @file PersonalizationSettings.vue
 * @description 设置应用的语言与主题偏好分区。
 */

import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { VustSelect } from '@/components/ui'
import { useThemeStore } from '@/stores/theme'
import SettingsSectionPanel from './SettingsSectionPanel.vue'

const props = defineProps<{
  kind: 'language' | 'theme'
}>()

const { t, locale } = useI18n()
const themeStore = useThemeStore()
const title = computed(() => t(`app.settings.${props.kind}.label`))
const description = computed(() => t(`app.settings.${props.kind}.description`))
const options = computed(() =>
  props.kind === 'language'
    ? [
        { value: 'zh', label: t('app.settings.language.zh') },
        { value: 'en', label: t('app.settings.language.en') },
      ]
    : [
        { value: 'light', label: t('app.settings.theme.light') },
        { value: 'dark', label: t('app.settings.theme.dark') },
      ],
)
const value = computed(() => (props.kind === 'language' ? locale.value : themeStore.currentTheme))

/** 保存当前个性化选项。 */
const updateValue = (nextValue: string | number | boolean | null) => {
  if (props.kind === 'language' && (nextValue === 'zh' || nextValue === 'en')) {
    locale.value = nextValue
    localStorage.setItem('vust_locale', nextValue)
  } else if (props.kind === 'theme' && (nextValue === 'light' || nextValue === 'dark')) {
    themeStore.setTheme(nextValue)
  }
}
</script>

<template>
  <SettingsSectionPanel :title="title" :page="`settings-${kind}`">
    <p class="description">{{ description }}</p>
    <VustSelect
      :id="`settings-${kind}-select`"
      class="select-control"
      :name="`settings${kind === 'language' ? 'Language' : 'Theme'}`"
      :aria-label="title"
      :model-value="value"
      :options="options"
      data-ui="personalization-select"
      @update:model-value="updateValue"
    />
  </SettingsSectionPanel>
</template>

<style scoped>
.description {
  margin: 0 0 var(--vdl-space-4);
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-body-sm);
}

.select-control {
  width: min(100%, 240px);
}
</style>
