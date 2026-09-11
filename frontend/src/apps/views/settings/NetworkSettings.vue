<script setup lang="ts">
/**
 * @file NetworkSettings.vue
 * @description Master 监听地址与节点默认回连地址设置。
 */

import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { vustApi } from '@/api/modules/vust'
import {
  VustAlert,
  VustButton,
  VustFormItem,
  VustInput,
  VustLoading,
  VustSelect,
} from '@/components/ui'
import { useToastStore } from '@/stores/toast'
import SettingsSectionPanel from './SettingsSectionPanel.vue'

const emit = defineEmits<{
  busyChange: [busy: boolean]
  dirtyChange: [dirty: boolean]
}>()

interface InterfaceOption {
  value: string
  label: string
}

const { t } = useI18n()
const toastStore = useToastStore()
const loading = ref(false)
const saving = ref(false)
const loadedSnapshot = ref('')
const hostError = ref('')
const portError = ref('')
const form = ref({ host: '', port: 0, publicHost: '' })
const fetchedInterfaces = ref<InterfaceOption[]>([])
const hasLoadedInterfaces = ref(false)
const busy = computed(() => loading.value || saving.value)
const dirty = computed(
  () => Boolean(loadedSnapshot.value) && JSON.stringify(form.value) !== loadedSnapshot.value,
)

watch(busy, (value) => emit('busyChange', value), { immediate: true })
watch(dirty, (value) => emit('dirtyChange', value), { immediate: true })

const interfaceOptions = computed<InterfaceOption[]>(() => {
  const items = hasLoadedInterfaces.value
    ? fetchedInterfaces.value
    : [
        { value: '::', label: '' },
        { value: '0.0.0.0', label: '' },
      ]
  return items.map((item) => {
    if (item.value === '::')
      return { value: item.value, label: t('app.settings.network.interfaceOptions.dualStack') }
    if (item.value === '0.0.0.0')
      return { value: item.value, label: t('app.settings.network.interfaceOptions.ipv4Only') }
    return item
  })
})

/** 加载监听配置和可用网卡地址。 */
const loadConfig = async () => {
  loading.value = true
  try {
    const [interfacesResponse, configResponse] = await Promise.all([
      vustApi.fetchInterfaces(),
      vustApi.fetchNetwork(),
    ])
    if (interfacesResponse.success && interfacesResponse.data) {
      fetchedInterfaces.value = interfacesResponse.data.map((item) => ({
        value: item.ip,
        label: item.label,
      }))
      hasLoadedInterfaces.value = true
    }
    if (!configResponse.success || !configResponse.data) {
      throw new Error(configResponse.message || t('app.settings.network.loadFailed'))
    }
    form.value = {
      host: configResponse.data.host,
      port: configResponse.data.port,
      publicHost: configResponse.data.publicHost || '',
    }
    loadedSnapshot.value = JSON.stringify(form.value)
  } catch (error) {
    console.error('Failed to load VUST network config', error)
    toastStore.error(t('app.settings.network.loadFailed'))
  } finally {
    loading.value = false
  }
}

/** 校验并保存网络配置，成功后跳转到服务返回的新地址。 */
const saveConfig = async () => {
  hostError.value = form.value.host.trim() ? '' : t('app.settings.network.validationHost')
  portError.value =
    Number.isInteger(form.value.port) && form.value.port >= 1 && form.value.port <= 65535
      ? ''
      : t('app.settings.network.validationPort')
  if (hostError.value || portError.value) return

  saving.value = true
  try {
    const response = await vustApi.updateNetwork({
      host: form.value.host.trim(),
      port: form.value.port,
      publicHost: form.value.publicHost.trim() || null,
    })
    if (!response.success || !response.data) throw new Error(response.message)
    loadedSnapshot.value = JSON.stringify(form.value)
    toastStore.success(t('app.settings.network.saveSuccess'))
    window.setTimeout(() => window.location.replace(response.data!.nextUrl), 400)
  } catch (error) {
    console.error('Failed to save VUST network config', error)
    toastStore.error(t('app.settings.network.saveFailed'))
  } finally {
    saving.value = false
  }
}

onMounted(() => void loadConfig())
</script>

<template>
  <SettingsSectionPanel :title="t('app.settings.network.label')" page="settings-network">
    <div class="network-form" data-ui="network-form">
      <p class="description">{{ t('app.settings.network.description') }}</p>
      <VustFormItem
        :label="t('app.settings.network.host')"
        for="settings-network-host"
        :error="hostError"
      >
        <VustSelect
          id="settings-network-host"
          v-model="form.host"
          name="settingsNetworkHost"
          :options="interfaceOptions"
        />
      </VustFormItem>
      <VustFormItem
        :label="t('app.settings.network.port')"
        for="settings-network-port"
        :error="portError"
      >
        <VustInput
          id="settings-network-port"
          v-model.number="form.port"
          name="settingsNetworkPort"
          type="number"
          :min="1"
          :max="65535"
        />
      </VustFormItem>
      <VustFormItem
        :label="t('app.settings.network.publicHost')"
        for="settings-network-public-host"
      >
        <VustInput
          id="settings-network-public-host"
          v-model="form.publicHost"
          name="settingsNetworkPublicHost"
          :placeholder="t('app.settings.network.publicHostPlaceholder')"
        />
      </VustFormItem>
      <VustAlert type="warning" :title="t('app.settings.network.warning')" show-icon />
      <VustButton
        type="primary"
        :loading="saving"
        :disabled="loading || !dirty"
        data-ui="network-save"
        @click="saveConfig"
      >
        {{ t('app.settings.network.save') }}
      </VustButton>
      <VustLoading :loading="loading" :text="t('app.settings.network.loading')" cover />
    </div>
  </SettingsSectionPanel>
</template>

<style scoped>
.network-form {
  position: relative;
  max-width: 620px;
  min-height: 100%;
}
.description {
  margin: 0 0 var(--vdl-space-4);
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-body-sm);
}
</style>
