<script setup lang="ts">
/**
 * @file SuiteCenterView.vue
 * @description Compose 套件中心入口，负责筛选、布局与组合领域组件。
 */

import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type {
  SuiteCatalogItem,
  SuiteInstallTaskResponse,
  SuiteInstanceSummary,
} from '@/api/interface/suites'
import { VustButton, VustEmpty, VustInput, VustLoading, VustSelect } from '@/components/ui'
import VustIcon from '@/components/icons/VustIcon.vue'
import { useNodeStore } from '@/stores/node'
import SuiteCard from './suite-center/SuiteCard.vue'
import SuiteDangerDialogs from './suite-center/SuiteDangerDialogs.vue'
import SuiteDetailDrawer from './suite-center/SuiteDetailDrawer.vue'
import SuiteImportDialog from './suite-center/SuiteImportDialog.vue'
import { useSuiteCenter } from './suite-center/useSuiteCenter'

type SuiteCategory = 'all' | 'tools' | 'other'
type SuiteStatusFilter = 'all' | 'available' | 'installed' | 'running' | 'error'

interface SuiteCardModel {
  suite: SuiteCatalogItem
  instance?: SuiteInstanceSummary
  task?: SuiteInstallTaskResponse
  category: SuiteCategory
  status: string
}

const { t } = useI18n()
const nodeStore = useNodeStore()
const {
  activeOperations,
  cancelInstall,
  catalog,
  currentNodeId,
  currentNodeUnavailable,
  deleteSuite,
  importSuite,
  installSuite,
  instances,
  isOperating,
  loadError,
  phase,
  pollingErrors,
  refreshSuites,
  refreshing,
  retryInstallPolling,
  runInstanceAction,
  tasksById,
} = useSuiteCenter()

const activeCategory = ref<SuiteCategory>('all')
const statusFilter = ref<SuiteStatusFilter>('all')
const searchQuery = ref('')
const selectedSuiteId = ref('')
const importDialogVisible = ref(false)
const deleteTarget = ref<SuiteCatalogItem | null>(null)
const uninstallTarget = ref<SuiteCardModel | null>(null)

const taskBySuite = computed(() => {
  const result = new Map<string, SuiteInstallTaskResponse>()
  Object.values(tasksById.value)
    .filter((task) => task.nodeId === currentNodeId.value && !task.isFinished)
    .forEach((task) => result.set(task.suiteId, task))
  return result
})

const suiteCards = computed<SuiteCardModel[]>(() =>
  catalog.value.map((suite) => {
    const instance = instances.value.find((item) => item.suiteId === suite.suiteId)
    const task = taskBySuite.value.get(suite.suiteId)
    return {
      suite,
      instance,
      task,
      category: resolveCategory(suite),
      status: instance?.status ?? task?.status ?? suite.status,
    }
  }),
)

const selectedCard = computed(
  () => suiteCards.value.find((card) => card.suite.suiteId === selectedSuiteId.value) ?? null,
)

const categoryItems = computed(() =>
  (['all', 'tools', 'other'] as SuiteCategory[]).map((key) => ({
    key,
    label: t(`app.suiteCenter.categories.${key}`),
    count:
      key === 'all'
        ? suiteCards.value.length
        : suiteCards.value.filter((card) => card.category === key).length,
  })),
)

const statusOptions = computed(() =>
  (['all', 'available', 'installed', 'running', 'error'] as SuiteStatusFilter[]).map((value) => ({
    value,
    label: t(`app.suiteCenter.filters.${value}`),
  })),
)

const filteredCards = computed(() => {
  const query = searchQuery.value.trim().toLocaleLowerCase()
  return suiteCards.value.filter((card) => {
    const categoryMatches = activeCategory.value === 'all' || card.category === activeCategory.value
    const searchMatches =
      !query ||
      [card.suite.name, card.suite.suiteId, card.suite.summary].some((value) =>
        value.toLocaleLowerCase().includes(query),
      )
    const statusMatches = matchesStatusFilter(card, statusFilter.value)
    return categoryMatches && searchMatches && statusMatches
  })
})

function resolveCategory(suite: SuiteCatalogItem): SuiteCategory {
  return suite.category?.toLowerCase() === 'tools' ? 'tools' : 'other'
}

function matchesStatusFilter(card: SuiteCardModel, filter: SuiteStatusFilter) {
  if (filter === 'all') return true
  if (filter === 'available') return !card.instance && !card.task
  if (filter === 'installed') return !!card.instance
  if (filter === 'running') return card.instance?.status === 'enabled'
  return card.instance?.status === 'error' || card.task?.status === 'failed'
}

function statusLabel(status: string) {
  const knownStatuses = new Set([
    'available',
    'installing',
    'installed',
    'enabling',
    'enabled',
    'disabling',
    'disabled',
    'uninstalling',
    'error',
    'queued',
    'running',
    'canceling',
    'failed',
    'canceled',
  ])
  return t(`app.suiteCenter.status.${knownStatuses.has(status) ? status : 'unknown'}`)
}

function statusType(status: string) {
  if (status === 'enabled') return 'success' as const
  if (status === 'error' || status === 'failed') return 'danger' as const
  if (['installed', 'disabled', 'canceled'].includes(status)) return 'default' as const
  if (['canceling', 'uninstalling', 'disabling'].includes(status)) return 'warning' as const
  return 'primary' as const
}

function nodeName(nodeId: string) {
  return nodeStore.nodes.find((node) => node.id === nodeId)?.name || nodeId
}

function closeDetail() {
  selectedSuiteId.value = ''
}

async function handleImport(file: File) {
  const result = await importSuite(file)
  if (result.valid) importDialogVisible.value = false
}

async function confirmDelete() {
  if (!deleteTarget.value) return
  if (await deleteSuite(deleteTarget.value)) {
    deleteTarget.value = null
    closeDetail()
  }
}

async function confirmUninstall(removeData: boolean) {
  const target = uninstallTarget.value
  if (!target?.instance) return
  if (await runInstanceAction(target.suite, target.instance, 'uninstall', removeData)) {
    uninstallTarget.value = null
    closeDetail()
  }
}

watch(catalog, () => {
  if (
    selectedSuiteId.value &&
    !catalog.value.some((suite) => suite.suiteId === selectedSuiteId.value)
  ) {
    closeDetail()
  }
})
</script>

<template>
  <div class="suite-center" data-page="suite-center">
    <aside class="suite-categories" data-slot="category-navigation">
      <div class="suite-categories__title">{{ t('app.suiteCenter.appName') }}</div>
      <div class="suite-categories__list" role="list" data-ui="suite-category-list">
        <button
          v-for="category in categoryItems"
          :key="category.key"
          type="button"
          class="suite-category"
          :class="{ 'is-active': activeCategory === category.key }"
          :aria-pressed="activeCategory === category.key"
          @click="activeCategory = category.key"
        >
          <span>{{ category.label }}</span>
          <span class="suite-category__count">{{ category.count }}</span>
        </button>
      </div>
    </aside>

    <main class="suite-main">
      <div class="suite-toolbar" data-ui="suite-toolbar">
        <VustInput
          v-model="searchQuery"
          :placeholder="t('app.suiteCenter.searchPlaceholder')"
          :aria-label="t('app.suiteCenter.searchPlaceholder')"
          data-slot="suite-search"
        />
        <VustSelect
          v-model="statusFilter"
          :options="statusOptions"
          :aria-label="t('app.suiteCenter.statusFilterLabel')"
          data-slot="suite-status-filter"
        />
        <VustButton
          class="suite-toolbar__action"
          type="secondary"
          :disabled="refreshing"
          data-ui="suite-refresh"
          @click="refreshSuites()"
        >
          <VustIcon name="refresh" :size="14" />
          {{ t('common.refresh') }}
        </VustButton>
        <VustButton
          class="suite-toolbar__action"
          type="primary"
          data-ui="suite-import"
          @click="importDialogVisible = true"
        >
          <VustIcon name="plus" :size="14" />
          {{ t('common.import') }}
        </VustButton>
      </div>

      <div
        class="suite-results"
        data-slot="suite-results"
        :aria-busy="refreshing || phase === 'loading'"
      >
        <div
          v-if="refreshing || phase === 'loading'"
          class="suite-state suite-state--loading"
          data-ui="suite-results-loading"
          aria-live="polite"
        >
          <VustLoading :loading="true" :text="t('app.suiteCenter.loading')" />
        </div>
        <div v-else-if="currentNodeUnavailable" class="suite-state" data-ui="node-unavailable">
          <VustEmpty :description="t('app.suiteCenter.nodeUnavailable')" icon="server" />
        </div>
        <div v-else-if="phase === 'error'" class="suite-state" data-ui="suite-load-error">
          <VustEmpty :description="loadError" icon="alert-circle" />
          <VustButton type="secondary" @click="refreshSuites()">{{ t('common.retry') }}</VustButton>
        </div>
        <div v-else-if="phase === 'empty'" class="suite-state" data-ui="suite-empty-catalog">
          <VustEmpty :description="t('app.suiteCenter.empty')" icon="package" />
        </div>
        <div
          v-else-if="filteredCards.length === 0"
          class="suite-state"
          data-ui="suite-empty-filter"
        >
          <VustEmpty :description="t('app.suiteCenter.emptyFilter')" icon="search" />
        </div>
        <div v-else class="suite-grid">
          <SuiteCard
            v-for="card in filteredCards"
            :key="card.suite.suiteId"
            :suite="card.suite"
            :instance="card.instance"
            :status-label="statusLabel(card.status)"
            :status-type="statusType(card.status)"
            @select="selectedSuiteId = card.suite.suiteId"
          />
        </div>
      </div>
    </main>

    <SuiteDetailDrawer
      :suite="selectedCard?.suite ?? null"
      :instance="selectedCard?.instance"
      :task="selectedCard?.task"
      :status-label="selectedCard ? statusLabel(selectedCard.status) : ''"
      :status-type="selectedCard ? statusType(selectedCard.status) : 'default'"
      :node-name="nodeName(currentNodeId)"
      :busy="selectedCard ? isOperating(selectedCard.suite.suiteId) : false"
      :node-unavailable="currentNodeUnavailable"
      :polling-error="selectedCard?.task ? pollingErrors[selectedCard.task.taskId] : undefined"
      @close="closeDetail"
      @install="selectedCard && installSuite(selectedCard.suite)"
      @enable="
        selectedCard?.instance &&
        runInstanceAction(selectedCard.suite, selectedCard.instance, 'enable')
      "
      @disable="
        selectedCard?.instance &&
        runInstanceAction(selectedCard.suite, selectedCard.instance, 'disable')
      "
      @uninstall="uninstallTarget = selectedCard"
      @delete="deleteTarget = selectedCard?.suite ?? null"
      @cancel="cancelInstall"
      @retry="retryInstallPolling"
    />

    <SuiteImportDialog
      :visible="importDialogVisible"
      :busy="!!activeOperations.import"
      @close="importDialogVisible = false"
      @submit="handleImport"
    />
    <SuiteDangerDialogs
      :delete-target="deleteTarget"
      :uninstall-suite="uninstallTarget?.suite ?? null"
      :uninstall-instance="uninstallTarget?.instance"
      :busy="
        !!(
          (deleteTarget && isOperating(deleteTarget.suiteId)) ||
          (uninstallTarget && isOperating(uninstallTarget.suite.suiteId))
        )
      "
      @close-delete="deleteTarget = null"
      @confirm-delete="confirmDelete"
      @close-uninstall="uninstallTarget = null"
      @confirm-uninstall="confirmUninstall"
    />
  </div>
</template>

<style scoped>
.suite-center {
  display: grid;
  grid-template-columns: minmax(10rem, 13rem) minmax(0, 1fr);
  width: 100%;
  height: 100%;
  min-height: 0;
  color: var(--vdl-text-primary);
  background: var(--vdl-bg-canvas);
}

.suite-categories {
  display: flex;
  min-height: 0;
  flex-direction: column;
  gap: var(--vdl-space-4);
  padding: var(--vdl-space-5) var(--vdl-space-3);
  border-right: 1px solid var(--vdl-border-subtle);
  background: var(--vdl-bg-panel);
}

.suite-categories__title {
  padding-inline: var(--vdl-space-2);
  font: var(--vdl-font-title);
}

.suite-categories__list {
  display: grid;
  gap: var(--vdl-space-1);
}

.suite-category {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3);
  border: 0;
  border-radius: var(--vdl-radius-md);
  color: var(--vdl-text-secondary);
  font: var(--vdl-font-body-sm);
  text-align: left;
  background: transparent;
  cursor: pointer;
}

.suite-category:hover {
  color: var(--vdl-text-primary);
  background: var(--vdl-bg-hover);
}

.suite-category.is-active {
  color: var(--vdl-primary);
  background: var(--vdl-bg-active);
}

.suite-category:focus-visible {
  outline: 2px solid var(--vdl-focus-ring);
  outline-offset: 1px;
}

.suite-category__count {
  color: var(--vdl-text-muted);
  font: var(--vdl-font-caption);
}

.suite-main {
  display: flex;
  min-width: 0;
  min-height: 0;
  flex-direction: column;
}

.suite-toolbar {
  display: grid;
  grid-template-columns: minmax(12rem, 1fr) minmax(9rem, 12rem) auto auto;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-4) var(--vdl-space-5);
  border-bottom: 1px solid var(--vdl-border-subtle);
  background: var(--vdl-bg-panel);
}

.suite-toolbar__action :deep(.vl-button-content) {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: var(--vdl-space-2);
  line-height: 1;
}

.suite-results {
  position: relative;
  min-height: 0;
  flex: 1;
  overflow: auto;
  padding: var(--vdl-space-5);
}

.suite-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(min(22rem, 100%), 1fr));
  gap: var(--vdl-space-4);
}

.suite-state {
  display: grid;
  place-items: center;
  gap: var(--vdl-space-3);
  height: 100%;
  min-height: 16rem;
}

.suite-state--loading {
  place-items: stretch;
}

@media (max-width: 680px) {
  .suite-center {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto minmax(0, 1fr);
  }

  .suite-categories {
    display: block;
    padding: var(--vdl-space-2) var(--vdl-space-3);
    border-right: 0;
    border-bottom: 1px solid var(--vdl-border-subtle);
  }

  .suite-categories__title {
    display: none;
  }

  .suite-categories__list {
    display: flex;
    overflow-x: auto;
  }

  .suite-category {
    flex: 0 0 auto;
    padding: var(--vdl-space-2) var(--vdl-space-3);
  }

  .suite-toolbar {
    grid-template-columns: minmax(0, 1fr) minmax(8rem, 0.7fr);
    padding: var(--vdl-space-3);
  }

  .suite-results {
    padding: var(--vdl-space-3);
  }

  .suite-grid {
    grid-template-columns: minmax(0, 1fr);
  }
}
</style>
