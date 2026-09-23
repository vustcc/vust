<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { NodePrecheckResponse } from '@/api/modules/nodes'
import {
  VustAlert,
  VustButton,
  VustDescriptions,
  VustFormItem,
  VustInput,
  VustSelect,
  VustTag,
} from '@/components/ui'

export type NodeProvisionPhase = 'create' | 'precheck' | 'deploy'

export interface NodeCreateFormModel {
  name: string
  groupId: string
  groupCustom: string
  description: string
  addr: string
  port: string
  user: string
  authMode: string
  pwd: string
  privateKey: string
  privateKeyPassphrase: string
  tags: string
  installDir: string
  servicePort: string
  vustUrl: string
}

interface Props {
  phase: NodeProvisionPhase
  form: NodeCreateFormModel
  precheckResult: NodePrecheckResponse | null
  precheckSubmitting: boolean
  deployLaunching: boolean
  deployRunning: boolean
  deployFinished: boolean
  deployTargetName: string
  deployProgressPercent: number
  deployLogs: string[]
  deployError: string
}

const props = defineProps<Props>()

const emit = defineEmits<{
  back: []
  precheck: []
  edit: []
  deploy: []
  background: []
}>()

const { t } = useI18n()
const form = computed(() => props.form)
const titleElement = ref<HTMLHeadingElement | null>(null)

/** 阶段切换后将键盘焦点移动到工作台标题。 */
async function focusTitle() {
  await nextTick()
  titleElement.value?.focus({ preventScroll: true })
}

onMounted(() => void focusTitle())
watch(() => props.phase, focusTitle)

/** 离开工作台后将焦点恢复到列表中的对应入口。 */
onBeforeUnmount(() => {
  const selector =
    props.phase === 'deploy' ? '[data-ui="node-deploy-resume"]' : '[data-ui="node-create"]'
  void nextTick(() => document.querySelector<HTMLElement>(selector)?.focus({ preventScroll: true }))
})

type PrecheckItemStatus = 'passed' | 'warning' | 'failed' | 'skipped'

interface PrecheckItem {
  key: string
  label: string
  status: PrecheckItemStatus
  message: string
  slot: string
}

const title = computed(() => {
  if (props.phase === 'precheck') return t('app.nodes.precheck.title')
  if (props.phase === 'deploy') return t('app.nodes.deploy.drawerTitle')
  return t('app.nodes.create.title')
})

const precheckItems = computed<PrecheckItem[]>(() => {
  if (!props.precheckResult) return []
  return [
    {
      key: 'ssh',
      label: t('app.nodes.precheck.indicators.ssh'),
      status: props.precheckResult.ssh.status,
      message: props.precheckResult.ssh.message,
      slot: 'ssh',
    },
    {
      key: 'callback',
      label: t('app.nodes.precheck.indicators.callback'),
      status: props.precheckResult.callback.status,
      message: props.precheckResult.callback.message,
      slot: 'callback',
    },
    {
      key: 'agentStatus',
      label: t('app.nodes.precheck.indicators.agentStatus'),
      status: props.precheckResult.agentStatus.blocking ? 'failed' : 'passed',
      message: props.precheckResult.agentStatus.message,
      slot: 'agentStatus',
    },
    {
      key: 'os',
      label: t('app.nodes.precheck.indicators.os'),
      status: props.precheckResult.os.status,
      message: props.precheckResult.os.message,
      slot: 'os',
    },
    {
      key: 'permission',
      label: t('app.nodes.precheck.indicators.permission'),
      status: props.precheckResult.permission.status,
      message: props.precheckResult.permission.message,
      slot: 'permission',
    },
    {
      key: 'service',
      label: t('app.nodes.precheck.indicators.service'),
      status: props.precheckResult.service.status,
      message: props.precheckResult.service.message,
      slot: 'service',
    },
    {
      key: 'systemd',
      label: t('app.nodes.precheck.indicators.systemd'),
      status: props.precheckResult.systemd.status,
      message: props.precheckResult.systemd.message,
      slot: 'systemd',
    },
    {
      key: 'directory',
      label: t('app.nodes.precheck.indicators.directory'),
      status: props.precheckResult.directory.status,
      message: props.precheckResult.directory.message,
      slot: 'directory',
    },
    {
      key: 'docker',
      label: t('app.nodes.precheck.indicators.docker'),
      status: props.precheckResult.docker.status,
      message: props.precheckResult.docker.message,
      slot: 'docker',
    },
    {
      key: 'port',
      label: t('app.nodes.precheck.indicators.port'),
      status: props.precheckResult.port.status,
      message: props.precheckResult.port.message,
      slot: 'port',
    },
  ]
})

const precheckPrimaryFailureMessage = computed(() => {
  if (props.precheckResult?.passed) return ''
  const serviceMessage = props.precheckResult?.service?.message || ''
  if (props.precheckResult?.service.status === 'failed' && serviceMessage) return serviceMessage
  return (
    precheckItems.value.find((item) => item.status === 'failed')?.message ||
    t('app.nodes.precheck.failed')
  )
})

const deployStatusType = computed(() => {
  if (props.deployError) return 'danger'
  if (props.deployFinished) return 'success'
  return 'warning'
})

const deployStatusText = computed(() => {
  if (props.deployError) return t('app.nodes.deploy.statusFailed')
  if (props.deployFinished) return t('app.nodes.deploy.statusSuccess')
  const progress = props.deployProgressPercent > 0 ? ` (${props.deployProgressPercent}%)` : ''
  return `${t('app.nodes.deploy.statusRunning')}${progress}`
})

function precheckStatusTagType(status: PrecheckItemStatus) {
  if (status === 'passed') return 'success'
  if (status === 'warning') return 'warning'
  if (status === 'skipped') return 'info'
  return 'danger'
}

function precheckStatusText(status: PrecheckItemStatus) {
  return t(`app.nodes.precheck.status.${status}Short`)
}
</script>

<template>
  <section class="node-provision-workspace" :data-ui="`node-${phase}-view`" :data-phase="phase">
    <header class="workspace-header" data-slot="header">
      <VustButton
        type="secondary"
        :disabled="precheckSubmitting || deployLaunching"
        :data-ui="
          phase === 'deploy' && deployFinished ? 'node-deploy-finish' : 'node-provision-back'
        "
        @click="emit('back')"
      >
        {{ t('common.back') }}
      </VustButton>
      <h2 ref="titleElement" tabindex="-1">{{ title }}</h2>
    </header>

    <div class="workspace-body" data-slot="body">
      <form v-if="phase === 'create'" class="node-create-form" @submit.prevent>
        <section class="form-section">
          <h3>{{ t('app.nodes.sections.basic') }}</h3>
          <div class="form-grid-2col">
            <VustFormItem :label="t('app.nodes.create.name')">
              <VustInput v-model="form.name" data-ui="node-create-name" />
            </VustFormItem>
            <VustFormItem :label="t('app.nodes.create.addr')">
              <VustInput v-model="form.addr" data-ui="node-create-address" />
            </VustFormItem>
            <VustFormItem :label="t('app.nodes.create.port')">
              <VustInput v-model="form.port" />
            </VustFormItem>
            <VustFormItem :label="t('app.nodes.create.user')">
              <VustInput v-model="form.user" autocomplete="username" />
            </VustFormItem>
          </div>
        </section>

        <section class="form-section">
          <h3>{{ t('app.nodes.sections.auth') }}</h3>
          <div class="form-grid-2col">
            <VustFormItem :label="t('app.nodes.create.authMode')">
              <VustSelect
                v-model="form.authMode"
                :options="[
                  { label: t('app.nodes.create.authPassword'), value: 'password' },
                  { label: t('app.nodes.create.authKey'), value: 'key' },
                ]"
              />
            </VustFormItem>
            <VustFormItem
              v-if="form.authMode === 'password'"
              :label="t('app.nodes.create.password')"
            >
              <input type="text" autocomplete="username" hidden />
              <VustInput
                v-model="form.pwd"
                type="password"
                show-password
                autocomplete="new-password"
              />
            </VustFormItem>
            <VustFormItem v-else :label="t('app.nodes.create.privateKeyPassphrase')">
              <input type="text" autocomplete="username" hidden />
              <VustInput
                v-model="form.privateKeyPassphrase"
                type="password"
                show-password
                autocomplete="new-password"
              />
            </VustFormItem>
            <VustFormItem
              v-if="form.authMode === 'key'"
              :label="t('app.nodes.create.privateKey')"
              class="grid-col-2"
            >
              <VustInput
                v-model="form.privateKey"
                :placeholder="t('app.nodes.create.privateKeyPlaceholder')"
              />
              <div class="form-hint">{{ t('app.nodes.create.privateKeyHint') }}</div>
            </VustFormItem>
          </div>
        </section>

        <section class="form-section">
          <h3>{{ t('app.nodes.sections.group') }}</h3>
          <div class="form-grid-2col">
            <VustFormItem :label="t('app.nodes.create.groupId')">
              <VustSelect
                v-model="form.groupId"
                :options="[
                  { label: t('app.nodes.create.groupDefault'), value: 'default' },
                  { label: t('app.nodes.create.groupCustom'), value: 'custom' },
                ]"
              />
            </VustFormItem>
            <VustFormItem
              v-if="form.groupId === 'custom'"
              :label="t('app.nodes.create.groupCustomInput')"
            >
              <VustInput v-model="form.groupCustom" />
            </VustFormItem>
            <VustFormItem :label="t('app.nodes.create.tags')">
              <VustInput v-model="form.tags" />
            </VustFormItem>
            <VustFormItem :label="t('app.nodes.create.description')" class="grid-col-2">
              <VustInput v-model="form.description" />
            </VustFormItem>
          </div>
        </section>

        <section class="form-section">
          <h3>{{ t('app.nodes.sections.install') }}</h3>
          <div class="form-grid-2col">
            <VustFormItem :label="t('app.nodes.create.installDir')">
              <VustInput
                v-model="form.installDir"
                :placeholder="t('app.nodes.create.installDirPlaceholder')"
              />
            </VustFormItem>
            <VustFormItem :label="t('app.nodes.create.servicePort')">
              <VustInput v-model="form.servicePort" />
            </VustFormItem>
            <VustFormItem :label="t('app.nodes.create.vustUrl')" class="grid-col-2">
              <VustInput
                v-model="form.vustUrl"
                :placeholder="t('app.nodes.create.vustUrlPlaceholder')"
              />
              <div class="form-hint">{{ t('app.nodes.create.vustUrlHint') }}</div>
            </VustFormItem>
          </div>
        </section>
      </form>

      <div v-else-if="phase === 'precheck'" class="precheck-content">
        <div class="section-heading">
          <span>{{ t('app.nodes.precheck.title') }}</span>
          <VustTag :type="precheckResult?.passed ? 'success' : 'danger'">
            {{
              precheckResult?.passed
                ? t('app.nodes.precheck.statusPassed')
                : t('app.nodes.precheck.statusFailed')
            }}
          </VustTag>
        </div>
        <VustAlert
          v-if="!precheckResult?.passed"
          :title="t('app.nodes.precheck.conflictTitle')"
          :description="precheckPrimaryFailureMessage"
          type="error"
          show-icon
        />
        <VustDescriptions :items="precheckItems" border>
          <template v-for="detail in precheckItems" :key="detail.key" #[detail.slot]>
            <div class="precheck-item-row">
              <VustTag :type="precheckStatusTagType(detail.status)">
                {{ precheckStatusText(detail.status) }}
              </VustTag>
              <span>{{ detail.message || '-' }}</span>
            </div>
          </template>
        </VustDescriptions>
      </div>

      <div v-else class="deploy-content">
        <VustDescriptions
          :items="[
            { label: t('app.nodes.deploy.target'), slot: 'target' },
            { label: t('app.nodes.deploy.statusLabel'), slot: 'status' },
          ]"
          border
        >
          <template #target>{{ deployTargetName || '-' }}</template>
          <template #status>
            <VustTag :type="deployStatusType">{{ deployStatusText }}</VustTag>
          </template>
        </VustDescriptions>
        <VustAlert v-if="deployError" :title="deployError" type="error" show-icon />
        <div class="logs-container" data-ui="node-deploy-logs">
          <div v-if="deployLogs.length === 0" class="deploy-empty">
            {{ deployError ? t('app.nodes.deploy.noLogs') : t('app.nodes.deploy.deploying') }}
          </div>
          <div v-for="(line, index) in deployLogs" :key="index" class="deploy-line">
            {{ line }}
          </div>
        </div>
      </div>
    </div>

    <footer
      v-if="phase !== 'deploy' || (deployRunning && !deployLaunching)"
      class="workspace-footer"
      data-slot="footer"
    >
      <template v-if="phase === 'create'">
        <VustButton
          type="primary"
          :loading="precheckSubmitting"
          data-ui="node-run-precheck"
          @click="emit('precheck')"
        >
          {{ t('app.nodes.create.submit') }}
        </VustButton>
      </template>
      <template v-else-if="phase === 'precheck'">
        <VustButton data-ui="node-precheck-edit" @click="emit('edit')">
          {{ t('app.nodes.precheck.backToEdit') }}
        </VustButton>
        <VustButton
          v-if="precheckResult?.passed"
          type="primary"
          data-ui="node-start-deploy"
          @click="emit('deploy')"
        >
          {{ t('app.nodes.precheck.confirmDeploy') }}
        </VustButton>
      </template>
      <template v-else>
        <VustButton
          v-if="deployRunning && !deployLaunching"
          type="secondary"
          data-ui="node-deploy-background"
          @click="emit('background')"
        >
          {{ t('app.nodes.deploy.runInBackground') }}
        </VustButton>
      </template>
    </footer>
  </section>
</template>

<style scoped>
.node-provision-workspace {
  display: flex;
  min-height: 0;
  flex: 1;
  flex-direction: column;
  overflow: hidden;
  border: 1px solid var(--vdl-border-default);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-panel);
}

.workspace-header,
.workspace-footer {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: var(--vdl-space-3);
  padding: var(--vdl-space-3) var(--vdl-space-4);
}

.workspace-header {
  border-bottom: 1px solid var(--vdl-border-subtle);
}

.workspace-header h2,
.form-section h3 {
  margin: 0;
  color: var(--vdl-text-primary);
}

.workspace-header h2 {
  font-size: var(--vdl-font-subtitle);
}

.workspace-body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: var(--vdl-space-4);
  scrollbar-gutter: stable;
}

.workspace-footer {
  justify-content: flex-end;
  border-top: 1px solid var(--vdl-border-subtle);
}

.node-create-form {
  display: grid;
  max-width: 960px;
  margin: 0 auto;
  gap: var(--vdl-space-5);
}

.form-section,
.precheck-content,
.deploy-content {
  display: grid;
  gap: var(--vdl-space-3);
}

.form-section h3 {
  padding-bottom: var(--vdl-space-2);
  border-bottom: 1px solid var(--vdl-border-subtle);
  font-size: var(--vdl-font-body);
}

.form-grid-2col {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--vdl-space-3) var(--vdl-space-5);
}

.grid-col-2 {
  grid-column: span 2;
}

.form-hint,
.deploy-empty {
  color: var(--vdl-text-muted);
  font-size: var(--vdl-font-caption);
}

.form-hint {
  margin-top: var(--vdl-space-1);
}

.section-heading,
.precheck-item-row {
  display: flex;
  align-items: center;
  gap: var(--vdl-space-2);
}

.section-heading {
  justify-content: space-between;
  color: var(--vdl-text-primary);
  font-weight: 600;
}

.precheck-item-row span:last-child {
  min-width: 0;
  overflow-wrap: anywhere;
}

.logs-container {
  min-height: 240px;
  max-height: 100%;
  overflow: auto;
  padding: var(--vdl-space-3);
  border: 1px solid var(--vdl-border-subtle);
  border-radius: var(--vdl-radius-md);
  background: var(--vdl-bg-muted);
  color: var(--vdl-text-secondary);
  font-family: var(--vdl-font-mono);
  font-size: var(--vdl-font-caption);
  line-height: 1.6;
}

.deploy-line {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

@media (max-width: 760px) {
  .form-grid-2col {
    grid-template-columns: 1fr;
  }

  .grid-col-2 {
    grid-column: auto;
  }
}
</style>
