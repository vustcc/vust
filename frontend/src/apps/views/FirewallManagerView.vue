<script setup lang="ts">
/**
 * @file FirewallManagerView.vue
 * @description 当前 Node 实际生效防火墙规则的只读观察视图。
 */

import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FirewallRuleSummary } from '@/api/modules/firewall'
import { useFirewallRules } from '@/composables/useFirewallRules'
import { useWindowManagerStore } from '@/stores/window-manager'
import {
  VustAlert,
  VustButton,
  VustDescriptions,
  VustDrawer,
  VustEmpty,
  VustInput,
  VustLoading,
  VustPagination,
  VustSelect,
  VustTable,
  VustTag,
} from '@/components/ui'
import type { VustTableColumn } from '@/components/ui/VustTable.vue'

const props = defineProps<{
  isMaximized?: boolean
  windowId?: string
  payload?: Record<string, unknown>
}>()

const { t } = useI18n()
const windowStore = useWindowManagerStore()
const manager = useFirewallRules()
const drawerVisible = ref(false)

watch(
  () => props.windowId,
  (windowId) => {
    if (!windowId) return
    windowStore.updateWindowRuntimeState(windowId, {
      busy: false,
      activeSession: false,
      allowsNodeSwitch: true,
      blockLevel: 'open',
      blockReason: t('app.firewallManager.guardOpen'),
    })
  },
  { immediate: true },
)

const engineOptions = computed(() => [
  { value: 'all', label: t('app.firewallManager.filters.allEngines') },
  { value: 'nftables', label: 'nftables' },
  { value: 'iptables', label: 'iptables' },
])
const familyOptions = computed(() => [
  { value: 'all', label: t('app.firewallManager.filters.allFamilies') },
  ...['inet', 'ipv4', 'ipv6', 'bridge', 'arp', 'netdev', 'unknown'].map((value) => ({
    value,
    label: t(`app.firewallManager.families.${value}`),
  })),
])
const actionOptions = computed(() => [
  { value: 'all', label: t('app.firewallManager.filters.allActions') },
  ...[
    'accept',
    'drop',
    'reject',
    'return',
    'jump',
    'goto',
    'log',
    'masquerade',
    'dnat',
    'snat',
    'redirect',
    'queue',
    'continue',
    'other',
  ].map((value) => ({ value, label: t(`app.firewallManager.actions.${value}`) })),
])

const columns = computed<VustTableColumn[]>(() => [
  {
    prop: 'engine',
    label: t('app.firewallManager.columns.engine'),
    width: 108,
    slot: 'engine',
  },
  {
    prop: 'family',
    label: t('app.firewallManager.columns.family'),
    width: 92,
    slot: 'family',
  },
  {
    prop: 'table',
    label: t('app.firewallManager.columns.scope'),
    minWidth: 160,
    slot: 'scope',
  },
  {
    prop: 'action',
    label: t('app.firewallManager.columns.action'),
    width: 112,
    slot: 'action',
  },
  {
    prop: 'protocol',
    label: t('app.firewallManager.columns.protocol'),
    width: 90,
    slot: 'protocol',
  },
  {
    prop: 'sourceAddress',
    label: t('app.firewallManager.columns.source'),
    minWidth: 150,
    slot: 'source',
  },
  {
    prop: 'destinationAddress',
    label: t('app.firewallManager.columns.destination'),
    minWidth: 180,
    slot: 'destination',
  },
  {
    prop: 'ruleId',
    label: t('app.firewallManager.columns.detail'),
    width: 88,
    align: 'center',
    slot: 'detail',
    fixed: 'right',
  },
])

const totalPages = computed(() =>
  Math.max(1, Math.ceil((manager.page.value?.total ?? 0) / manager.pageSize.value)),
)
const hasFilters = computed(
  () =>
    Boolean(manager.keyword.value.trim()) ||
    manager.engine.value !== 'all' ||
    manager.family.value !== 'all' ||
    manager.action.value !== 'all',
)
const collectionTag = computed(() =>
  manager.page.value?.collection.status === 'partial'
    ? { type: 'warning' as const, label: t('app.firewallManager.collection.partial') }
    : { type: 'success' as const, label: t('app.firewallManager.collection.complete') },
)
const detailItems = computed(() => {
  const summary = manager.detail.value?.summary
  if (!summary) return []
  return [
    {
      label: t('app.firewallManager.detail.kind'),
      value: t(`app.firewallManager.kinds.${summary.kind}`),
    },
    { label: t('app.firewallManager.columns.engine'), value: summary.engine },
    {
      label: t('app.firewallManager.columns.family'),
      value: t(`app.firewallManager.families.${summary.family}`),
    },
    { label: t('app.firewallManager.detail.table'), value: summary.table },
    { label: t('app.firewallManager.detail.chain'), value: summary.chain },
    { label: t('app.firewallManager.detail.position'), value: summary.position },
    { label: t('app.firewallManager.columns.action'), value: formatAction(summary) },
    summary.protocol
      ? { label: t('app.firewallManager.columns.protocol'), value: summary.protocol }
      : null,
    summary.sourceAddress
      ? { label: t('app.firewallManager.detail.sourceAddress'), value: summary.sourceAddress }
      : null,
    summary.destinationAddress
      ? {
          label: t('app.firewallManager.detail.destinationAddress'),
          value: summary.destinationAddress,
        }
      : null,
    summary.sourcePorts.length
      ? {
          label: t('app.firewallManager.detail.sourcePorts'),
          value: summary.sourcePorts.join(', '),
        }
      : null,
    summary.destinationPorts.length
      ? {
          label: t('app.firewallManager.detail.destinationPorts'),
          value: summary.destinationPorts.join(', '),
        }
      : null,
    summary.inputInterface
      ? { label: t('app.firewallManager.detail.inputInterface'), value: summary.inputInterface }
      : null,
    summary.outputInterface
      ? { label: t('app.firewallManager.detail.outputInterface'), value: summary.outputInterface }
      : null,
    summary.comment
      ? { label: t('app.firewallManager.detail.comment'), value: summary.comment }
      : null,
  ].filter((item): item is NonNullable<typeof item> => item !== null)
})

function actionTagType(action: FirewallRuleSummary['action']) {
  if (action === 'accept') return 'success'
  if (action === 'drop' || action === 'reject') return 'danger'
  if (action === 'jump' || action === 'goto' || action === 'continue') return 'info'
  return 'default'
}

function formatAction(rule: FirewallRuleSummary) {
  const label = t(`app.firewallManager.actions.${rule.action}`)
  return rule.actionTarget ? `${label} → ${rule.actionTarget}` : label
}

function formatEndpoint(address: string | undefined, ports: string[]) {
  if (!address && ports.length === 0) return t('app.firewallManager.any')
  if (!address) return ports.join(', ')
  if (ports.length === 0) return address
  return `${address}:${ports.join(', ')}`
}

function openDetail(rule: FirewallRuleSummary) {
  if (!rule.capabilities.canViewDetail) return
  drawerVisible.value = true
  void manager.loadDetail(rule)
}

function closeDetail() {
  drawerVisible.value = false
  manager.clearDetail()
}
</script>

<template>
  <div class="firewall-view" data-page="firewall-manager" data-ui="firewall-rules">
    <div class="toolbar" data-ui="toolbar" data-slot="toolbar">
      <div class="filters" data-slot="filters">
        <label class="visually-hidden" for="firewall-rule-search">
          {{ t('app.firewallManager.filters.searchLabel') }}
        </label>
        <VustInput
          id="firewall-rule-search"
          v-model="manager.keyword.value"
          class="search-input"
          name="firewallRuleSearch"
          :aria-label="t('app.firewallManager.filters.searchLabel')"
          :placeholder="t('app.firewallManager.filters.searchPlaceholder')"
        />

        <label class="visually-hidden" for="firewall-engine-filter">
          {{ t('app.firewallManager.filters.engineLabel') }}
        </label>
        <VustSelect
          id="firewall-engine-filter"
          v-model="manager.engine.value"
          class="filter-select"
          name="firewallEngineFilter"
          :aria-label="t('app.firewallManager.filters.engineLabel')"
          :options="engineOptions"
        />

        <label class="visually-hidden" for="firewall-family-filter">
          {{ t('app.firewallManager.filters.familyLabel') }}
        </label>
        <VustSelect
          id="firewall-family-filter"
          v-model="manager.family.value"
          class="filter-select"
          name="firewallFamilyFilter"
          :aria-label="t('app.firewallManager.filters.familyLabel')"
          :options="familyOptions"
        />

        <label class="visually-hidden" for="firewall-action-filter">
          {{ t('app.firewallManager.filters.actionLabel') }}
        </label>
        <VustSelect
          id="firewall-action-filter"
          v-model="manager.action.value"
          class="filter-select"
          name="firewallActionFilter"
          :aria-label="t('app.firewallManager.filters.actionLabel')"
          :options="actionOptions"
        />
      </div>

      <VustButton
        type="secondary"
        class="refresh-button"
        :loading="manager.manualRefreshing.value"
        data-slot="refresh"
        @click="manager.refresh"
      >
        <span class="refresh-label">
          {{
            manager.manualRefreshing.value
              ? t('app.firewallManager.refreshing')
              : t('app.firewallManager.refresh')
          }}
        </span>
      </VustButton>
    </div>

    <VustAlert
      v-if="manager.warning.value"
      type="warning"
      :title="t('app.firewallManager.messages.stale')"
      :description="manager.warning.value"
      show-icon
      data-ui="refresh-warning"
    />
    <VustAlert
      v-if="manager.page.value?.collection.status === 'partial'"
      type="warning"
      :title="
        t('app.firewallManager.collection.partialCoverage', {
          coverage: manager.page.value.collection.coveragePercent.toFixed(0),
        })
      "
      show-icon
      data-ui="partial-collection"
    />

    <div class="content" data-slot="content">
      <div
        v-if="manager.phase.value === 'initialLoading'"
        class="center-state initial-loading-state"
        aria-busy="true"
        data-ui="initial-loading"
      >
        <VustLoading
          class="initial-loading"
          :loading="true"
          :text="t('app.firewallManager.loading')"
        />
      </div>

      <div
        v-else-if="manager.phase.value === 'initialError'"
        class="center-state error-state"
        data-ui="initial-error"
      >
        <VustEmpty icon="error" data-slot="error-empty">
          <span class="error-title">{{ t('app.firewallManager.messages.initialError') }}</span>
          <template #extra>
            <div class="error-actions">
              <VustButton type="secondary" @click="manager.retry">
                {{ t('app.firewallManager.retry') }}
              </VustButton>
            </div>
          </template>
        </VustEmpty>
      </div>

      <VustEmpty
        v-else-if="manager.page.value?.availableTotal === 0"
        :description="t('app.firewallManager.empty')"
        data-ui="rules-empty"
      />

      <div v-else class="table-workspace" data-ui="table-workspace">
        <div class="table-meta" data-slot="status">
          <div class="status-cluster">
            <VustTag :type="collectionTag.type" effect="light">
              {{ collectionTag.label }}
            </VustTag>
            <span>
              {{
                t('app.firewallManager.ruleCount', {
                  filtered: manager.page.value?.total ?? 0,
                  total: manager.page.value?.availableTotal ?? 0,
                })
              }}
            </span>
          </div>
          <span v-if="manager.loadedAt.value" class="collected-at">
            {{
              t('app.firewallManager.collectedAt', {
                time: manager.loadedAt.value.toLocaleString(),
              })
            }}
          </span>
        </div>

        <div class="table-region" data-slot="table">
          <VustTable
            :data="manager.rules.value"
            :columns="columns"
            row-key="ruleId"
            border
            data-ui="table"
          >
            <template #engine="{ row }">
              <VustTag type="info" effect="light">{{ row.engine }}</VustTag>
            </template>
            <template #family="{ row }">
              {{ t(`app.firewallManager.families.${row.family}`) }}
            </template>
            <template #scope="{ row }">
              <span class="code-text">{{ row.table }}/{{ row.chain }}</span>
            </template>
            <template #action="{ row }">
              <VustTag :type="actionTagType(row.action)" effect="light">
                {{ formatAction(row) }}
              </VustTag>
            </template>
            <template #protocol="{ row }">
              <span v-if="row.protocol" class="code-text">{{ row.protocol }}</span>
            </template>
            <template #source="{ row }">
              <span class="code-text">{{
                formatEndpoint(row.sourceAddress, row.sourcePorts)
              }}</span>
            </template>
            <template #destination="{ row }">
              <span class="code-text">
                {{ formatEndpoint(row.destinationAddress, row.destinationPorts) }}
              </span>
            </template>
            <template #detail="{ row }">
              <VustButton
                type="secondary"
                size="small"
                :disabled="!row.capabilities.canViewDetail"
                :aria-label="t('app.firewallManager.detail.openAria')"
                @click="openDetail(row)"
              >
                {{ t('app.firewallManager.detail.open') }}
              </VustButton>
            </template>
            <template #empty>
              <VustEmpty
                :description="
                  hasFilters
                    ? t('app.firewallManager.filteredEmpty')
                    : t('app.firewallManager.empty')
                "
                data-ui="filtered-empty"
              />
            </template>
          </VustTable>
        </div>

        <div class="pagination-bar" data-slot="pagination">
          <VustPagination
            :current-page="manager.currentPage.value"
            :total-pages="totalPages"
            :aria-label="t('app.firewallManager.pagination.label')"
            :previous-label="t('app.firewallManager.pagination.previous')"
            :next-label="t('app.firewallManager.pagination.next')"
            data-ui="pagination"
            @page-change="manager.goToPage"
          />
        </div>
      </div>
    </div>

    <VustDrawer
      v-model="drawerVisible"
      :title="t('app.firewallManager.detail.title')"
      width="520px"
      data-ui="rule-detail-drawer"
      @close="closeDetail"
    >
      <div data-slot="detail">
        <VustLoading
          v-if="manager.detailLoading.value"
          :loading="true"
          :text="t('app.firewallManager.detail.loading')"
        />
        <VustAlert
          v-else-if="manager.detailError.value"
          type="error"
          :title="manager.detailError.value"
          show-icon
          data-ui="detail-error"
        />
        <template v-else-if="manager.detail.value">
          <VustAlert
            v-if="manager.detail.value.parseStatus === 'partial'"
            type="warning"
            :title="t('app.firewallManager.detail.partial')"
            show-icon
            data-ui="detail-partial"
          />
          <VustDescriptions :items="detailItems" :column="1" border />

          <div v-if="manager.detail.value.matches.length" class="detail-group" data-slot="matches">
            <div class="detail-heading">{{ t('app.firewallManager.detail.matches') }}</div>
            <div
              v-for="(match, index) in manager.detail.value.matches"
              :key="`${match.field}-${index}`"
              class="expression-row"
            >
              <span class="code-text">{{ match.field }}</span>
              <span>{{ match.operator }}</span>
              <span class="code-text">{{ match.value }}</span>
            </div>
          </div>

          <div v-if="manager.detail.value.effects.length" class="detail-group" data-slot="effects">
            <div class="detail-heading">{{ t('app.firewallManager.detail.effects') }}</div>
            <div
              v-for="(effect, index) in manager.detail.value.effects"
              :key="`${effect.action}-${index}`"
              class="expression-row"
            >
              <VustTag :type="actionTagType(effect.action)" effect="light">
                {{ t(`app.firewallManager.actions.${effect.action}`) }}
              </VustTag>
              <span v-if="effect.target" class="code-text">{{ effect.target }}</span>
            </div>
          </div>
        </template>
      </div>
    </VustDrawer>
  </div>
</template>

<style scoped>
.firewall-view {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: var(--vdl-bg-panel);
}

.toolbar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3) var(--vdl-space-4);
  border-bottom: 1px solid var(--vdl-border-subtle);
}

.filters {
  min-width: 0;
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
  flex-wrap: wrap;
}

.search-input {
  width: min(280px, 34vw);
}

.filter-select {
  width: 150px;
}

.refresh-button {
  flex: 0 0 auto;
}

.refresh-label {
  display: inline-block;
  white-space: nowrap;
}

.content {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  padding: var(--vdl-space-4);
  overflow: hidden;
}

.center-state {
  flex: 1;
  min-height: 0;
  display: flex;
  align-items: center;
  justify-content: center;
}

.error-state {
  flex-direction: column;
}

.initial-loading-state,
.initial-loading {
  width: 100%;
  min-width: 0;
  max-width: 100%;
}

.initial-loading :deep(.vl-loading-spinner),
.initial-loading :deep(.vl-loading-text) {
  min-width: 0;
  max-width: 100%;
}

.initial-loading :deep(.vl-loading-text) {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.error-title {
  display: block;
}

.error-title {
  margin-bottom: var(--vdl-space-2);
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-subtitle);
  font-weight: 600;
}

.error-actions {
  display: flex;
  flex-direction: column;
  align-items: center;
}

.table-workspace {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.table-meta {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-3);
  padding-bottom: var(--vdl-space-3);
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-body-sm);
}

.status-cluster {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
}

.collected-at {
  color: var(--vdl-text-muted);
}

.table-region {
  flex: 1;
  min-height: 0;
  overflow: auto;
}

.pagination-bar {
  flex-shrink: 0;
  padding-top: var(--vdl-space-3);
}

.code-text {
  font-family: var(--vdl-font-mono);
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-body-sm);
  overflow-wrap: anywhere;
}

.detail-group {
  margin-top: var(--vdl-space-5);
}

.detail-heading {
  margin-bottom: var(--vdl-space-2);
  color: var(--vdl-text-primary);
  font-size: var(--vdl-font-subtitle);
  font-weight: 600;
}

.expression-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--vdl-space-2);
  padding: var(--vdl-space-2) 0;
  border-bottom: 1px solid var(--vdl-border-subtle);
  color: var(--vdl-text-secondary);
}

.visually-hidden {
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

:deep(.vl-table-container) {
  min-width: 980px;
}

@media (max-width: 760px) {
  .toolbar {
    align-items: flex-start;
    flex-direction: column;
  }

  .filters,
  .search-input,
  .filter-select {
    width: 100%;
  }

  .table-meta {
    align-items: flex-start;
    flex-direction: column;
  }

  .content {
    padding: var(--vdl-space-3);
  }
}
</style>
