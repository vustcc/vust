<script setup lang="ts">
/**
 * @file AboutSettings.vue
 * @description 展示当前节点的软件、硬件与系统信息。
 */

import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { SystemAboutInfo } from '@/api/interface/system'
import { systemApi } from '@/api/modules/system'
import { VustButton, VustDescriptions, VustLoading } from '@/components/ui'
import { useNodeStore } from '@/stores/node'
import { useToastStore } from '@/stores/toast'
import { formatDateTime } from '@/utils/time'
import SettingsSectionPanel from './SettingsSectionPanel.vue'

const emit = defineEmits<{
  busyChange: [busy: boolean]
}>()

const { t } = useI18n()
const nodeStore = useNodeStore()
const toastStore = useToastStore()
const appVersion = import.meta.env.VITE_APP_VERSION ?? 'unknown'
const gitHash = import.meta.env.VITE_GIT_HASH ?? 'unknown'
const loading = ref(false)
const copying = ref(false)
const aboutInfo = ref<SystemAboutInfo | null>(null)
const nodeId = computed(() => nodeStore.currentNodeId || 'local')
const client = computed(() => systemApi.forNode(nodeId.value))
const busy = computed(() => loading.value || copying.value)

watch(busy, (value) => emit('busyChange', value), { immediate: true })

const textOrPlaceholder = (value?: string) => value?.trim() || '--'

const formatBytes = (bytes?: number) => {
  if (!bytes || bytes <= 0) return '--'
  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB']
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1)
  const value = bytes / Math.pow(1024, index)
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[index]}`
}

const formatUptime = (seconds?: number) => {
  if (!seconds || seconds <= 0) return '--'
  const days = Math.floor(seconds / 86400)
  const hours = Math.floor((seconds % 86400) / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  if (days > 0) return `${days}d ${hours}h ${minutes}m`
  if (hours > 0) return `${hours}h ${minutes}m`
  return `${minutes}m`
}

const formatCpuFreq = (freqMhz?: number) => {
  if (!freqMhz || freqMhz <= 0) return '--'
  return freqMhz >= 1000 ? `${(freqMhz / 1000).toFixed(2)} GHz` : `${freqMhz} MHz`
}

const formatList = (values?: string[]) => (values?.length ? values.join(', ') : '--')

const cpuText = () => {
  if (!aboutInfo.value) return '--'
  const cores =
    aboutInfo.value.cpuLogicalCores > 0
      ? ` (${t('app.settings.about.logicalCores', { count: aboutInfo.value.cpuLogicalCores })})`
      : ''
  return `${textOrPlaceholder(aboutInfo.value.cpuModel)}${cores}`
}

const softwareInfoItems = computed(() => [
  { label: t('app.settings.about.version'), value: appVersion },
  { label: t('app.settings.about.hash'), value: gitHash },
])

const hardwareInfoItems = computed(() => {
  const info = aboutInfo.value
  return [
    { label: t('app.settings.about.hostname'), value: textOrPlaceholder(info?.hostname) },
    { label: t('app.settings.about.osName'), value: textOrPlaceholder(info?.osName) },
    { label: t('app.settings.about.osVersion'), value: textOrPlaceholder(info?.osVersion) },
    { label: t('app.settings.about.kernelVersion'), value: textOrPlaceholder(info?.kernelVersion) },
    { label: t('app.settings.about.architecture'), value: textOrPlaceholder(info?.architecture) },
    { label: t('app.settings.about.cpu'), value: cpuText() },
    { label: t('app.settings.about.cpuVendor'), value: textOrPlaceholder(info?.cpuVendor) },
    { label: t('app.settings.about.cpuPhysicalCores'), value: info?.cpuPhysicalCores || '--' },
    { label: t('app.settings.about.cpuLogicalCores'), value: info?.cpuLogicalCores || '--' },
    { label: t('app.settings.about.cpuFrequency'), value: formatCpuFreq(info?.cpuFrequencyMhz) },
    { label: t('app.settings.about.cpuCacheL3'), value: textOrPlaceholder(info?.cpuCacheL3) },
    { label: t('app.settings.about.memory'), value: formatBytes(info?.memoryTotalBytes) },
    {
      label: t('app.settings.about.memoryAvailable'),
      value: formatBytes(info?.memoryAvailableBytes),
    },
    { label: t('app.settings.about.swapTotal'), value: formatBytes(info?.swapTotalBytes) },
    { label: t('app.settings.about.swapUsed'), value: formatBytes(info?.swapUsedBytes) },
    { label: t('app.settings.about.uptime'), value: formatUptime(info?.uptimeSeconds) },
    { label: t('app.settings.about.bootTime'), value: formatDateTime(info?.bootTimeEpoch) },
    { label: t('app.settings.about.timezone'), value: textOrPlaceholder(info?.timezone) },
    { label: t('app.settings.about.locale'), value: textOrPlaceholder(info?.locale) },
    {
      label: t('app.settings.about.virtualization'),
      value: textOrPlaceholder(info?.virtualization),
    },
    {
      label: t('app.settings.about.primaryInterface'),
      value: textOrPlaceholder(info?.primaryInterface),
    },
    { label: t('app.settings.about.macAddress'), value: textOrPlaceholder(info?.macAddress) },
    { label: t('app.settings.about.ipv4Addresses'), value: formatList(info?.ipv4Addresses) },
    { label: t('app.settings.about.ipv6Addresses'), value: formatList(info?.ipv6Addresses) },
    {
      label: t('app.settings.about.defaultGateway'),
      value: textOrPlaceholder(info?.defaultGateway),
    },
    { label: t('app.settings.about.dnsServers'), value: formatList(info?.dnsServers) },
    { label: t('app.settings.about.disk'), value: formatBytes(info?.diskTotalBytes) },
  ]
})

/** 加载当前节点系统信息，并忽略节点快速切换产生的过期响应。 */
const loadAboutInfo = async () => {
  const requestedNodeId = nodeId.value
  loading.value = true
  try {
    const response = await client.value.fetchAbout()
    if (requestedNodeId !== nodeId.value) return
    if (!response.success) throw new Error(response.message)
    aboutInfo.value = response.data ?? null
  } catch (error) {
    if (requestedNodeId !== nodeId.value) return
    console.error('Failed to load system about info', error)
    toastStore.error(t('app.settings.about.loadFailed'))
  } finally {
    if (requestedNodeId === nodeId.value) loading.value = false
  }
}

/** 复制可用于支持诊断的当前系统信息。 */
const copyAboutInfo = async () => {
  copying.value = true
  const text = JSON.stringify(
    {
      exportedAt: new Date().toISOString(),
      app: { version: appVersion, gitHash },
      system: aboutInfo.value,
    },
    null,
    2,
  )
  try {
    await navigator.clipboard.writeText(text)
    toastStore.success(t('app.settings.about.copySuccess'))
  } catch (error) {
    console.error('Failed to copy system about info', error)
    toastStore.error(t('app.settings.about.copyFailed'))
  } finally {
    copying.value = false
  }
}

watch(nodeId, () => void loadAboutInfo(), { immediate: true })
</script>

<template>
  <SettingsSectionPanel :title="t('app.settings.about.label')" page="settings-about">
    <template #actions>
      <VustButton size="small" :loading="copying" :disabled="!aboutInfo" @click="copyAboutInfo">
        {{ t('app.settings.about.copyButton') }}
      </VustButton>
    </template>
    <div class="about-content" data-ui="about-details">
      <p class="description">{{ t('app.settings.about.description') }}</p>
      <h3>{{ t('app.settings.about.software') }}</h3>
      <VustDescriptions :items="softwareInfoItems" border class="descriptions" />
      <h3>{{ t('app.settings.about.hardware') }}</h3>
      <VustDescriptions :items="hardwareInfoItems" border />
      <VustLoading :loading="loading" :text="t('app.settings.about.loading')" cover />
    </div>
  </SettingsSectionPanel>
</template>

<style scoped>
.about-content {
  position: relative;
  min-height: 100%;
}

.description {
  margin: 0 0 var(--vdl-space-4);
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-body-sm);
}

h3 {
  margin: 0 0 var(--vdl-space-2);
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-body-sm);
  font-weight: 600;
}

.descriptions {
  margin-bottom: var(--vdl-space-4);
}
</style>
