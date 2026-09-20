<script setup lang="ts">
/**
 * @file PersonalizationSettings.vue
 * @description 设置应用的语言与主题偏好分区。
 */

import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { VustSelect, VustSwitch } from '@/components/ui'
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

/** 保存 Liquid Glass 开关。 */
const updateGlass = (enabled: boolean) => {
  themeStore.setGlassEnabled(enabled)
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
    <div v-if="kind === 'theme'" class="material-setting" data-ui="glass-preference">
      <div class="material-copy">
        <strong>{{ t('app.settings.theme.glassLabel') }}</strong>
        <span>{{ t('app.settings.theme.glassDescription') }}</span>
      </div>
      <VustSwitch
        :model-value="themeStore.glassEnabled"
        :aria-label="t('app.settings.theme.glassLabel')"
        @update:model-value="updateGlass"
      />
    </div>
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

.material-setting {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-4);
  margin-top: var(--vdl-space-5);
  padding-top: var(--vdl-space-4);
  border-top: 1px solid var(--vdl-border-subtle);
}

.material-copy {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: var(--vdl-space-1);
}

.material-copy strong {
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-body-sm);
}

.material-copy span {
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-caption);
}
</style>
