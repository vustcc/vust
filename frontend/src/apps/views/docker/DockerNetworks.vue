<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { VustTableColumn } from '@/components/ui/VustTable.vue'
import {
  VustActionMenu,
  VustAlert,
  VustButton,
  VustDescriptions,
  VustDialog,
  VustDrawer,
  VustEmpty,
  VustFormItem,
  VustInput,
  VustLoading,
  VustSelect,
  VustSwitch,
  VustTable,
  VustTag,
} from '@/components/ui'
import type {
  DockerNetworkContainer,
  DockerNetworkCreateRequest,
  DockerNetworkManagementKind,
  DockerNetworkSummary,
} from '@/api/interface/docker'
import { useConfirmationModalStore } from '@/stores/confirmation-modal'
import { useDockerStore } from '@/stores/docker'
import { useNodeStore } from '@/stores/node'
import { formatKeyValue, parseKeyValueLines } from '@/utils/docker-format'
import { formatDateTime } from '@/utils/time'

defineOptions({ inheritAttrs: false })

/** Docker 网络列表、Bridge 创建和按需详情管理页。 */
const { t } = useI18n()
const store = useDockerStore()
const nodeStore = useNodeStore()
const modalStore = useConfirmationModalStore()

const search = ref('')
const managementFilter = ref<'' | DockerNetworkManagementKind>('')
const driverFilter = ref('')
const createVisible = ref(false)
const advancedVisible = ref(false)
const createError = ref('')
const detailVisible = ref(false)
const selectedSummary = ref<DockerNetworkSummary | null>(null)
const selectedContainerToConnect = ref('')

const emptyForm = () => ({
  name: '',
  ipv4Subnet: '',
  ipv4Gateway: '',
  ipv4Range: '',
  enableIpv6: false,
  ipv6Subnet: '',
  ipv6Gateway: '',
  ipv6Range: '',
  internal: false,
  options: '',
  labels: '',
})
const form = ref(emptyForm())

const columns = computed<VustTableColumn[]>(() => [
  { label: t('app.docker.networks.columns.name'), minWidth: 210, slot: 'name' },
  { label: t('app.docker.networks.columns.management'), width: 130, slot: 'management' },
  { label: t('app.docker.networks.columns.driver'), width: 110, slot: 'driver', align: 'center' },
  { label: t('app.docker.networks.columns.subnets'), minWidth: 210, slot: 'subnets' },
  { label: t('app.docker.networks.columns.actions'), width: 110, align: 'center', slot: 'actions' },
])

const managementOptions = computed(() => [
  { label: t('app.docker.networks.filters.allManagement'), value: '' },
  ...(['system', 'compose', 'suite', 'custom'] as const).map((value) => ({
    label: t(`app.docker.networks.management.${value}`),
    value,
  })),
])
const driverOptions = computed(() => [
  { label: t('app.docker.networks.filters.allDrivers'), value: '' },
  ...Array.from(new Set(store.networks.map((network) => network.driver)))
    .sort()
    .map((driver) => ({ label: driver, value: driver })),
])

const filteredNetworks = computed(() => {
  const keyword = search.value.trim().toLowerCase()
  return store.networks.filter((network) => {
    if (keyword && !network.name.toLowerCase().includes(keyword)) return false
    if (managementFilter.value && network.management.kind !== managementFilter.value) return false
    if (driverFilter.value && network.driver !== driverFilter.value) return false
    return true
  })
})

const currentDetail = computed(() => store.networkDetail)
const currentSummary = computed(() => currentDetail.value?.summary ?? selectedSummary.value)
const isSystemNetwork = computed(() => currentSummary.value?.management.kind === 'system')
const canManageConnections = computed(
  () => currentSummary.value?.capabilities.canManageConnections ?? false,
)

const containerColumns = computed<VustTableColumn[]>(() => {
  const columns: VustTableColumn[] = [
    { label: t('app.docker.networks.detail.name'), minWidth: 150, slot: 'name' },
    { label: t('app.docker.networks.detail.ipv4'), minWidth: 150, slot: 'ipv4' },
    { label: t('app.docker.networks.detail.ipv6'), minWidth: 180, slot: 'ipv6' },
    { label: t('app.docker.networks.detail.macAddress'), minWidth: 150, slot: 'mac' },
  ]
  if (canManageConnections.value) {
    columns.push({
      label: t('app.docker.networks.detail.actions'),
      width: 110,
      align: 'center',
      slot: 'actions',
    })
  }
  return columns
})

const connectableContainers = computed(() => {
  const connectedIds = new Set(
    currentDetail.value?.containers.map((container) => container.id) ?? [],
  )
  return store.containers
    .filter((container) => container.state === 'running' && !connectedIds.has(container.id))
    .map((container) => {
      return { label: container.name, value: container.id }
    })
})

const detailItems = computed(() => {
  const summary = currentSummary.value
  if (!summary) return []
  return [
    { label: t('app.docker.networks.detail.name'), value: summary.name },
    { label: t('app.docker.networks.detail.id'), value: summary.id },
    { label: t('app.docker.networks.detail.management'), value: managementLabel(summary) },
    { label: t('app.docker.networks.detail.driver'), value: summary.driver },
    { label: t('app.docker.networks.detail.scope'), value: summary.scope },
    { label: t('app.docker.networks.detail.createdAt'), value: formatDateTime(summary.createdAt) },
    {
      label: t('app.docker.networks.detail.ipam'),
      value: formatIpam(currentDetail.value?.ipamConfigs ?? []),
      span: 2,
    },
    { label: t('app.docker.networks.detail.features'), value: featureLabel(summary) },
    {
      label: t('app.docker.networks.detail.options'),
      value: formatKeyValue(currentDetail.value?.options),
    },
    {
      label: t('app.docker.networks.detail.labels'),
      value: formatKeyValue(currentDetail.value?.labels),
      span: 2,
    },
  ]
})

function managementLabel(network: DockerNetworkSummary): string {
  const label = t(`app.docker.networks.management.${network.management.kind}`)
  return network.management.ownerName ? `${label} · ${network.management.ownerName}` : label
}

/** 将网络的特殊能力转换为面向用户的本地化说明。 */
function featureLabel(network: DockerNetworkSummary): string {
  const features = [
    network.internal ? t('app.docker.networks.features.internal') : '',
    network.enableIpv6 ? t('app.docker.networks.features.ipv6') : '',
    network.attachable ? t('app.docker.networks.features.attachable') : '',
    network.ingress ? t('app.docker.networks.features.ingress') : '',
    network.configOnly ? t('app.docker.networks.features.configOnly') : '',
  ].filter(Boolean)
  return features.join(' · ') || '-'
}

function managementTagType(
  kind: DockerNetworkManagementKind,
): 'default' | 'primary' | 'warning' | 'info' {
  if (kind === 'custom') return 'primary'
  if (kind === 'compose') return 'info'
  if (kind === 'suite') return 'warning'
  return 'default'
}

/** 返回系统网络的操作限制说明。 */
function systemProtectionReason(): string {
  return t('app.docker.networks.systemProtected')
}

function formatIpam(
  configs: Array<{ subnet?: string; gateway?: string; ipRange?: string }>,
): string {
  if (!configs.length) return '-'
  return configs
    .map((config) => [config.subnet, config.gateway, config.ipRange].filter(Boolean).join(' · '))
    .filter(Boolean)
    .join('; ')
}

function openCreate(): void {
  form.value = emptyForm()
  advancedVisible.value = false
  createError.value = ''
  createVisible.value = true
}

function closeCreate(): void {
  if (store.networkCreateLoading) return
  createVisible.value = false
  createError.value = ''
}

function optionalIpam(subnet: string, gateway: string, ipRange: string) {
  const values = {
    subnet: subnet.trim() || undefined,
    gateway: gateway.trim() || undefined,
    ipRange: ipRange.trim() || undefined,
  }
  return Object.values(values).some(Boolean) ? values : undefined
}

function parseFormMap(value: string, label: string): Record<string, string> | undefined {
  const parsed = parseKeyValueLines(value)
  if (parsed === null) throw new Error(t('app.docker.networks.create.invalidKeyValue', { label }))
  return Object.keys(parsed).length ? parsed : undefined
}

async function submitCreate(): Promise<void> {
  createError.value = ''
  const name = form.value.name.trim()
  if (!name) {
    createError.value = t('app.docker.messages.networkNameRequired')
    return
  }
  try {
    const options = parseFormMap(form.value.options, t('app.docker.networks.create.options'))
    const labels = parseFormMap(form.value.labels, t('app.docker.networks.create.labels'))
    const reserved = Object.keys(labels ?? {}).find(
      (key) => key.startsWith('vust.') || key.startsWith('com.docker.compose.'),
    )
    if (reserved) {
      createError.value = t('app.docker.networks.create.reservedLabel', { key: reserved })
      return
    }
    const payload: DockerNetworkCreateRequest = {
      name,
      internal: form.value.internal,
      enableIpv6: form.value.enableIpv6,
      ipv4: optionalIpam(form.value.ipv4Subnet, form.value.ipv4Gateway, form.value.ipv4Range),
      ipv6: form.value.enableIpv6
        ? optionalIpam(form.value.ipv6Subnet, form.value.ipv6Gateway, form.value.ipv6Range)
        : undefined,
      options,
      labels,
    }
    if (await store.createNetwork(payload)) createVisible.value = false
  } catch (error) {
    createError.value = error instanceof Error ? error.message : t('common.unknownError')
  }
}

async function openDetail(network: DockerNetworkSummary): Promise<void> {
  selectedSummary.value = network
  selectedContainerToConnect.value = ''
  detailVisible.value = true
  await Promise.all([store.fetchNetworkDetail(network.id), store.fetchContainers()])
}

function closeDetail(): void {
  detailVisible.value = false
  selectedSummary.value = null
  selectedContainerToConnect.value = ''
  store.clearNetworkDetail()
}

async function deleteNetwork(network: DockerNetworkSummary): Promise<void> {
  if (!network.capabilities.canRemove) return
  const confirmed = await modalStore.showConfirmation(
    t(
      network.management.kind === 'custom'
        ? 'app.docker.networks.actions.deleteConfirm'
        : 'app.docker.networks.actions.deleteManagedConfirm',
      { name: network.name },
    ),
    t('app.docker.messages.deleteConfirmTitle'),
    t('app.docker.messages.deleteAction'),
    t('confirmation.cancel'),
  )
  if (!confirmed) return
  const deleted = await store.removeNetwork(network)
  if (deleted && selectedSummary.value?.id === network.id) closeDetail()
}

async function connectContainer(): Promise<void> {
  const networkId = currentSummary.value?.id
  if (!networkId || !selectedContainerToConnect.value) return
  if (await store.connectNetwork(networkId, selectedContainerToConnect.value)) {
    selectedContainerToConnect.value = ''
  }
}

async function disconnectContainer(
  container: DockerNetworkContainer,
  force: boolean,
): Promise<void> {
  const summary = currentSummary.value
  if (!summary || !summary.capabilities.canManageConnections) return
  const confirmed = await modalStore.showConfirmation(
    t(
      force
        ? 'app.docker.networks.detail.forceDisconnectConfirm'
        : 'app.docker.networks.detail.disconnectConfirm',
      { containerName: container.name, networkName: summary.name },
    ),
    t(
      force
        ? 'app.docker.networks.detail.forceDisconnectTitle'
        : 'app.docker.networks.detail.disconnectTitle',
    ),
    t(
      force
        ? 'app.docker.networks.detail.forceDisconnectAction'
        : 'app.docker.networks.detail.disconnectContainer',
    ),
    t('confirmation.cancel'),
  )
  if (confirmed) await store.disconnectNetwork(summary.id, container.id, force)
}

function rowActions(network: DockerNetworkSummary) {
  return [
    {
      label: t('app.docker.networks.actions.view'),
      handler: () => void openDetail(network),
    },
    {
      label: t('app.docker.networks.actions.delete'),
      handler: () => void deleteNetwork(network),
      class: 'app-btn-delete',
      disabled: !network.capabilities.canRemove,
      tooltip: network.capabilities.canRemove ? undefined : systemProtectionReason(),
    },
  ]
}

function containerActions(container: DockerNetworkContainer) {
  return [
    {
      label: t('app.docker.networks.detail.disconnectContainer'),
      handler: () => void disconnectContainer(container, false),
    },
    {
      label: t('app.docker.networks.detail.forceDisconnectAction'),
      handler: () => void disconnectContainer(container, true),
      class: 'app-btn-delete',
    },
  ]
}

watch(
  () => nodeStore.currentNodeId,
  () => void store.fetchNetworks(),
  { immediate: true },
)
</script>

<template>
  <div class="network-page" data-page="docker-networks">
    <div class="network-toolbar" data-ui="toolbar">
      <div class="toolbar-filters" data-slot="filters">
        <label class="sr-only" for="docker-network-search">
          {{ t('app.docker.networks.filters.searchPlaceholder') }}
        </label>
        <VustInput
          id="docker-network-search"
          v-model="search"
          class="search-input"
          :placeholder="t('app.docker.networks.filters.searchPlaceholder')"
        />
        <VustSelect v-model="managementFilter" class="filter-select" :options="managementOptions" />
        <VustSelect v-model="driverFilter" class="filter-select" :options="driverOptions" />
      </div>
      <div class="toolbar-actions" data-slot="actions">
        <VustButton :loading="store.networkListLoading" @click="store.fetchNetworks">
          {{ t('common.refresh') }}
        </VustButton>
        <VustButton type="primary" @click="openCreate">
          {{ t('app.docker.networks.actions.create') }}
        </VustButton>
      </div>
    </div>

    <VustAlert
      v-if="store.networkListError"
      type="warning"
      :title="t('app.docker.networks.refreshFailed')"
      :description="store.networkListError"
      show-icon
      data-ui="network-list-error"
    />

    <div class="network-table-shell" data-ui="table">
      <VustTable v-if="filteredNetworks.length" :data="filteredNetworks" :columns="columns" border>
        <template #name="{ row }: { row: DockerNetworkSummary }">
          <button class="network-name" type="button" @click="openDetail(row)">
            {{ row.name }}
          </button>
        </template>
        <template #management="{ row }: { row: DockerNetworkSummary }">
          <VustTag :type="managementTagType(row.management.kind)" size="small">
            {{ managementLabel(row) }}
          </VustTag>
        </template>
        <template #driver="{ row }: { row: DockerNetworkSummary }">
          <span class="mono-text">{{ row.driver }}</span>
        </template>
        <template #subnets="{ row }: { row: DockerNetworkSummary }">
          <span class="mono-text subnet-cell">{{ row.subnets.join(', ') || '-' }}</span>
        </template>
        <template #actions="{ row }: { row: DockerNetworkSummary }">
          <VustActionMenu
            :label="t('app.docker.networks.actions.menu')"
            :actions="rowActions(row)"
          />
        </template>
      </VustTable>
      <VustEmpty
        v-else-if="!store.networkListLoading"
        :description="
          search || managementFilter || driverFilter
            ? t('app.docker.networks.filteredEmpty')
            : t('app.docker.networks.empty')
        "
      />
      <VustLoading :loading="store.networkListLoading && !store.networks.length" cover />
    </div>

    <VustDialog
      :visible="createVisible"
      :title="t('app.docker.networks.create.title')"
      width="680px"
      :close-on-click-overlay="!store.networkCreateLoading"
      data-ui="network-create-dialog"
      @close="closeCreate"
    >
      <div class="create-form" data-slot="body">
        <VustAlert v-if="createError" type="error" :title="createError" show-icon />
        <div class="form-grid">
          <VustFormItem
            :label="t('app.docker.networks.create.name')"
            for="docker-network-name"
            required
          >
            <VustInput
              id="docker-network-name"
              v-model="form.name"
              :placeholder="t('app.docker.networks.create.namePlaceholder')"
            />
          </VustFormItem>
          <VustFormItem :label="t('app.docker.networks.create.driver')" for="docker-network-driver">
            <VustInput id="docker-network-driver" model-value="bridge" readonly />
          </VustFormItem>
        </div>
        <VustButton type="secondary" size="small" @click="advancedVisible = !advancedVisible">
          {{
            advancedVisible
              ? t('app.docker.networks.create.hideAdvanced')
              : t('app.docker.networks.create.showAdvanced')
          }}
        </VustButton>
        <div v-if="advancedVisible" class="advanced-fields" data-slot="advanced">
          <div class="form-group-title">{{ t('app.docker.networks.create.ipv4Title') }}</div>
          <div class="form-grid three-columns">
            <VustFormItem
              :label="t('app.docker.networks.create.subnet')"
              for="docker-network-ipv4-subnet"
            >
              <VustInput
                id="docker-network-ipv4-subnet"
                v-model="form.ipv4Subnet"
                :placeholder="t('app.docker.networks.create.ipv4SubnetPlaceholder')"
              />
            </VustFormItem>
            <VustFormItem
              :label="t('app.docker.networks.create.gateway')"
              for="docker-network-ipv4-gateway"
            >
              <VustInput
                id="docker-network-ipv4-gateway"
                v-model="form.ipv4Gateway"
                :placeholder="t('app.docker.networks.create.ipv4GatewayPlaceholder')"
              />
            </VustFormItem>
            <VustFormItem
              :label="t('app.docker.networks.create.ipRange')"
              for="docker-network-ipv4-range"
            >
              <VustInput
                id="docker-network-ipv4-range"
                v-model="form.ipv4Range"
                :placeholder="t('app.docker.networks.create.ipv4RangePlaceholder')"
              />
            </VustFormItem>
          </div>
          <div class="switch-row">
            <VustSwitch
              v-model="form.enableIpv6"
              :active-text="t('app.docker.networks.create.enableIpv6')"
            />
            <VustSwitch
              v-model="form.internal"
              :active-text="t('app.docker.networks.create.internal')"
            />
          </div>
          <template v-if="form.enableIpv6">
            <div class="form-group-title">{{ t('app.docker.networks.create.ipv6Title') }}</div>
            <div class="form-grid three-columns">
              <VustFormItem
                :label="t('app.docker.networks.create.subnet')"
                for="docker-network-ipv6-subnet"
              >
                <VustInput
                  id="docker-network-ipv6-subnet"
                  v-model="form.ipv6Subnet"
                  :placeholder="t('app.docker.networks.create.ipv6SubnetPlaceholder')"
                />
              </VustFormItem>
              <VustFormItem
                :label="t('app.docker.networks.create.gateway')"
                for="docker-network-ipv6-gateway"
              >
                <VustInput
                  id="docker-network-ipv6-gateway"
                  v-model="form.ipv6Gateway"
                  :placeholder="t('app.docker.networks.create.ipv6GatewayPlaceholder')"
                />
              </VustFormItem>
              <VustFormItem
                :label="t('app.docker.networks.create.ipRange')"
                for="docker-network-ipv6-range"
              >
                <VustInput
                  id="docker-network-ipv6-range"
                  v-model="form.ipv6Range"
                  :placeholder="t('app.docker.networks.create.ipv6RangePlaceholder')"
                />
              </VustFormItem>
            </div>
          </template>
          <VustFormItem
            :label="t('app.docker.networks.create.options')"
            :hint="t('app.docker.networks.create.keyValueHint')"
            for="docker-network-options"
          >
            <VustInput
              id="docker-network-options"
              v-model="form.options"
              type="textarea"
              :rows="3"
            />
          </VustFormItem>
          <VustFormItem
            :label="t('app.docker.networks.create.labels')"
            :hint="t('app.docker.networks.create.labelHint')"
            for="docker-network-labels"
          >
            <VustInput id="docker-network-labels" v-model="form.labels" type="textarea" :rows="3" />
          </VustFormItem>
        </div>
      </div>
      <template #footer>
        <VustButton :disabled="store.networkCreateLoading" @click="closeCreate">{{
          t('common.cancel')
        }}</VustButton>
        <VustButton type="primary" :loading="store.networkCreateLoading" @click="submitCreate">{{
          t('app.docker.networks.actions.submit')
        }}</VustButton>
      </template>
    </VustDialog>

    <VustDrawer
      v-model="detailVisible"
      :title="t('app.docker.networks.detail.title')"
      width="780px"
      data-ui="network-detail-drawer"
      @close="closeDetail"
    >
      <div class="detail-content" data-slot="detail">
        <VustAlert
          v-if="isSystemNetwork"
          type="info"
          :title="t('app.docker.networks.detail.systemProtectedTitle')"
          :description="systemProtectionReason()"
          show-icon
        />
        <VustAlert
          v-if="store.networkDetailError"
          type="warning"
          :title="t('app.docker.networks.detail.refreshFailed')"
          :description="store.networkDetailError"
          show-icon
        />
        <VustDescriptions v-if="currentSummary" :items="detailItems" :column="2" border />

        <div class="detail-block">
          <div class="block-title">{{ t('app.docker.networks.detail.containers') }}</div>
          <VustTable
            v-if="currentDetail?.containers.length"
            :data="currentDetail.containers"
            :columns="containerColumns"
            border
          >
            <template #name="{ row }: { row: DockerNetworkContainer }"
              ><span class="container-name">{{ row.name }}</span></template
            >
            <template #ipv4="{ row }: { row: DockerNetworkContainer }"
              ><span class="mono-text">{{ row.ipv4Address || '-' }}</span></template
            >
            <template #ipv6="{ row }: { row: DockerNetworkContainer }"
              ><span class="mono-text">{{ row.ipv6Address || '-' }}</span></template
            >
            <template #mac="{ row }: { row: DockerNetworkContainer }"
              ><span class="mono-text">{{ row.macAddress || '-' }}</span></template
            >
            <template #actions="{ row }: { row: DockerNetworkContainer }">
              <VustActionMenu
                v-if="canManageConnections"
                :label="t('app.docker.networks.actions.menu')"
                :disabled="store.networkDisconnectLoading[row.id]"
                :actions="containerActions(row)"
              />
            </template>
          </VustTable>
          <VustEmpty
            v-else-if="!store.networkDetailLoading"
            :description="t('app.docker.networks.detail.noContainers')"
          />
        </div>

        <div
          v-if="canManageConnections"
          class="detail-block connect-block"
          data-slot="connect-container"
        >
          <div class="block-title">{{ t('app.docker.networks.detail.connectContainer') }}</div>
          <div class="connect-row">
            <VustSelect
              v-model="selectedContainerToConnect"
              :options="connectableContainers"
              :placeholder="t('app.docker.networks.detail.selectContainerPlaceholder')"
            />
            <VustButton
              type="primary"
              :disabled="!selectedContainerToConnect"
              :loading="store.networkConnectLoading"
              @click="connectContainer"
            >
              {{ t('app.docker.networks.detail.connectAction') }}
            </VustButton>
          </div>
          <VustEmpty
            v-if="!connectableContainers.length"
            :description="t('app.docker.networks.detail.noConnectableContainers')"
          />
        </div>
        <VustLoading :loading="store.networkDetailLoading && !currentDetail" cover />
      </div>
    </VustDrawer>
  </div>
</template>

<style scoped>
.network-page {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
  padding-top: var(--vdl-space-3);
}
.network-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-4);
  padding: var(--vdl-space-3) var(--vdl-space-4);
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
}
.toolbar-filters,
.toolbar-actions,
.switch-row,
.connect-row {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
}
.search-input {
  width: 240px;
}
.filter-select {
  width: 150px;
}
.network-table-shell {
  position: relative;
  flex: 1;
  min-height: 260px;
  overflow: auto;
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
}
.network-name {
  padding: 0;
  border: 0;
  background: transparent;
  color: var(--vdl-primary);
  font: inherit;
  font-weight: 600;
  cursor: pointer;
}
.network-name:hover {
  text-decoration: underline;
}
.mono-text {
  font-family: var(--vdl-font-mono);
}
.subnet-cell {
  white-space: normal;
  word-break: break-word;
}
.create-form,
.advanced-fields,
.detail-content,
.detail-block {
  display: flex;
  flex-direction: column;
  gap: var(--vdl-space-3);
}
.form-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 0 var(--vdl-space-4);
}
.three-columns {
  grid-template-columns: repeat(3, minmax(0, 1fr));
}
.advanced-fields {
  margin-top: var(--vdl-space-2);
  padding-top: var(--vdl-space-3);
  border-top: 1px solid var(--vdl-border-subtle);
}
.form-group-title,
.block-title {
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-body);
  font-weight: 600;
}
.detail-content {
  position: relative;
  min-height: 240px;
}
.detail-block {
  padding-top: var(--vdl-space-3);
  border-top: 1px solid var(--vdl-border-subtle);
}
.container-name {
  font-weight: 600;
}
.connect-row > :first-child {
  flex: 1;
}
.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}
@media (max-width: 980px) {
  .network-toolbar {
    align-items: stretch;
    flex-direction: column;
  }
  .toolbar-filters,
  .toolbar-actions {
    flex-wrap: wrap;
  }
}
@media (max-width: 680px) {
  .search-input,
  .filter-select {
    width: 100%;
  }
  .toolbar-filters,
  .toolbar-actions,
  .connect-row {
    align-items: stretch;
    flex-direction: column;
  }
  .form-grid,
  .three-columns {
    grid-template-columns: 1fr;
  }
}
</style>
