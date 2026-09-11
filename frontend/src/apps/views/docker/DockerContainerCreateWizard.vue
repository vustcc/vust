<script setup lang="ts">
/**
 * @file DockerContainerCreateWizard.vue
 * @description 使用结构化字段创建自定义 Docker 容器。
 */

import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useDockerStore } from '@/stores/docker'
import {
  VustAlert,
  VustButton,
  VustCheckbox,
  VustDialog,
  VustFormItem,
  VustInput,
  VustSelect,
} from '@/components/ui'
import type { DockerContainerCreateMount, DockerContainerCreatePort } from '@/api/interface/docker'

const { t } = useI18n()
const store = useDockerStore()
const advancedVisible = ref(false)

const imageOptions = computed(() =>
  store.imagesList.map((image) => ({
    value: image.id,
    label: image.tags[0] || image.id.replace(/^sha256:/, '').slice(0, 12),
  })),
)
const networkOptions = computed(() => [
  { label: t('app.docker.containers.default'), value: '' },
  ...store.networks.map((network) => ({ label: network.name, value: network.id })),
])
const restartPolicyOptions = computed(() => [
  { label: t('app.docker.containers.restartPolicies.no'), value: 'no' },
  { label: t('app.docker.containers.restartPolicies.onFailure'), value: 'on_failure' },
  { label: t('app.docker.containers.restartPolicies.always'), value: 'always' },
  { label: t('app.docker.containers.restartPolicies.unlessStopped'), value: 'unless_stopped' },
])
const mountKindOptions = computed(() => [
  { label: t('app.docker.containers.mountKinds.volume'), value: 'volume' },
  { label: t('app.docker.containers.mountKinds.bind'), value: 'bind' },
])
const protocolOptions = [
  { label: 'TCP', value: 'tcp' },
  { label: 'UDP', value: 'udp' },
  { label: 'SCTP', value: 'sctp' },
]

function textValue(value: unknown): string {
  return value === null || value === undefined ? '' : String(value)
}

function numberValue(value: unknown): number {
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : 0
}

function addEnvironment(): void {
  store.containerForm.environment.push({ name: '', value: '' })
}

function addPort(): void {
  store.containerForm.ports.push({
    hostPort: 8080,
    containerPort: 80,
    protocol: 'tcp',
  })
}

function addMount(): void {
  store.containerForm.mounts.push({
    kind: 'volume',
    source: '',
    target: '/data',
    readOnly: false,
  })
}

function updatePort(index: number, update: Partial<DockerContainerCreatePort>): void {
  store.containerForm.ports[index] = { ...store.containerForm.ports[index], ...update }
}

function updateMount(index: number, update: Partial<DockerContainerCreateMount>): void {
  store.containerForm.mounts[index] = { ...store.containerForm.mounts[index], ...update }
}
</script>

<template>
  <VustDialog
    :visible="store.isContainerCreateActive"
    :title="t('app.docker.containers.createContainer')"
    width="820px"
    :close-on-click-overlay="!store.containerCreateLoading"
    data-ui="container-create-dialog"
    @close="store.cancelContainerCreate"
  >
    <div class="create-form" data-slot="body">
      <VustAlert
        v-if="store.containerCreateError"
        type="error"
        :title="store.containerCreateError"
        show-icon
      />
      <VustAlert
        v-if="imageOptions.length === 0"
        type="warning"
        :title="t('app.docker.containers.noImagesForCreate')"
        show-icon
      />

      <div class="form-grid">
        <VustFormItem
          :label="t('app.docker.containers.selectImage')"
          for="docker-container-image"
          required
        >
          <VustSelect
            id="docker-container-image"
            :model-value="store.selectedImageId || ''"
            :options="imageOptions"
            :placeholder="t('app.docker.containers.selectImagePrompt')"
            @update:model-value="(value) => (store.selectedImageId = textValue(value) || null)"
          />
        </VustFormItem>
        <VustFormItem
          :label="t('app.docker.containers.containerName')"
          for="docker-container-name"
          required
        >
          <VustInput
            id="docker-container-name"
            v-model="store.containerForm.name"
            name="docker-container-name"
            :placeholder="t('app.docker.containers.containerNamePlaceholder')"
          />
        </VustFormItem>
      </div>

      <VustFormItem
        :label="t('app.docker.containers.startCommand')"
        :hint="t('app.docker.containers.startCommandHint')"
        for="docker-container-command"
      >
        <VustInput
          id="docker-container-command"
          v-model="store.containerForm.command"
          name="docker-container-command"
          :placeholder="t('app.docker.containers.startCommandPlaceholder')"
        />
      </VustFormItem>

      <VustButton type="secondary" size="small" @click="advancedVisible = !advancedVisible">
        {{
          advancedVisible
            ? t('app.docker.containers.hideAdvanced')
            : t('app.docker.containers.showAdvanced')
        }}
      </VustButton>

      <div v-if="advancedVisible" class="advanced-fields" data-slot="advanced">
        <div class="field-group">
          <div class="group-heading">
            <span>{{ t('app.docker.containers.envVars') }}</span>
            <VustButton size="small" type="secondary" @click="addEnvironment">
              {{ t('app.docker.containers.add') }}
            </VustButton>
          </div>
          <div
            v-for="(variable, index) in store.containerForm.environment"
            :key="`environment-${index}`"
            class="repeat-row two-columns"
          >
            <VustInput
              :id="`docker-container-env-name-${index}`"
              v-model="variable.name"
              :name="`docker-container-env-name-${index}`"
              :placeholder="t('app.docker.containers.varName')"
            />
            <VustInput
              :id="`docker-container-env-value-${index}`"
              v-model="variable.value"
              :name="`docker-container-env-value-${index}`"
              :placeholder="t('app.docker.containers.varValue')"
            />
            <VustButton size="small" @click="store.containerForm.environment.splice(index, 1)">
              {{ t('app.docker.containers.remove') }}
            </VustButton>
          </div>
        </div>

        <div class="field-group">
          <div class="group-heading">
            <span>{{ t('app.docker.containers.portMappings') }}</span>
            <VustButton size="small" type="secondary" @click="addPort">
              {{ t('app.docker.containers.add') }}
            </VustButton>
          </div>
          <div
            v-for="(port, index) in store.containerForm.ports"
            :key="`port-${index}`"
            class="repeat-row port-columns"
          >
            <VustInput
              :id="`docker-container-host-port-${index}`"
              :model-value="port.hostPort || ''"
              :name="`docker-container-host-port-${index}`"
              type="number"
              :placeholder="t('app.docker.containers.hostPort')"
              @update:model-value="(value) => updatePort(index, { hostPort: numberValue(value) })"
            />
            <VustInput
              :id="`docker-container-target-port-${index}`"
              :model-value="port.containerPort"
              :name="`docker-container-target-port-${index}`"
              type="number"
              :placeholder="t('app.docker.containers.containerPort')"
              @update:model-value="
                (value) => updatePort(index, { containerPort: numberValue(value) })
              "
            />
            <VustSelect
              :model-value="port.protocol"
              :options="protocolOptions"
              @update:model-value="
                (value) =>
                  updatePort(index, {
                    protocol: textValue(value) as DockerContainerCreatePort['protocol'],
                  })
              "
            />
            <VustButton size="small" @click="store.containerForm.ports.splice(index, 1)">
              {{ t('app.docker.containers.remove') }}
            </VustButton>
          </div>
        </div>

        <div class="field-group">
          <div class="group-heading">
            <span>{{ t('app.docker.containers.volumeMounts') }}</span>
            <VustButton size="small" type="secondary" @click="addMount">
              {{ t('app.docker.containers.add') }}
            </VustButton>
          </div>
          <div
            v-for="(mount, index) in store.containerForm.mounts"
            :key="`mount-${index}`"
            class="repeat-row mount-columns"
          >
            <VustSelect
              :model-value="mount.kind"
              :options="mountKindOptions"
              @update:model-value="
                (value) =>
                  updateMount(index, {
                    kind: textValue(value) as DockerContainerCreateMount['kind'],
                  })
              "
            />
            <VustInput
              :id="`docker-container-mount-source-${index}`"
              v-model="mount.source"
              :name="`docker-container-mount-source-${index}`"
              :placeholder="t('app.docker.containers.mountSource')"
            />
            <VustInput
              :id="`docker-container-mount-target-${index}`"
              v-model="mount.target"
              :name="`docker-container-mount-target-${index}`"
              :placeholder="t('app.docker.containers.containerPath')"
            />
            <VustCheckbox
              :model-value="mount.readOnly"
              @update:model-value="(value) => updateMount(index, { readOnly: value })"
            >
              {{ t('app.docker.containers.readOnlyMount') }}
            </VustCheckbox>
            <VustButton size="small" @click="store.containerForm.mounts.splice(index, 1)">
              {{ t('app.docker.containers.remove') }}
            </VustButton>
          </div>
        </div>

        <div class="form-grid three-columns">
          <VustFormItem :label="t('app.docker.containers.restartPolicy')">
            <VustSelect
              v-model="store.containerForm.restartPolicy"
              :options="restartPolicyOptions"
            />
          </VustFormItem>
          <VustFormItem
            v-if="store.containerForm.restartPolicy === 'on_failure'"
            :label="t('app.docker.containers.maximumRetryCount')"
            for="docker-container-retry-count"
          >
            <VustInput
              id="docker-container-retry-count"
              :model-value="store.containerForm.maximumRetryCount ?? ''"
              name="docker-container-retry-count"
              type="number"
              @update:model-value="
                (value) => (store.containerForm.maximumRetryCount = numberValue(value))
              "
            />
          </VustFormItem>
          <VustFormItem :label="t('app.docker.containers.network')">
            <VustSelect v-model="store.containerForm.networkId" :options="networkOptions" />
          </VustFormItem>
        </div>

        <VustCheckbox v-model="store.containerForm.autoRemove">
          {{ t('app.docker.containers.autoRemove') }}
        </VustCheckbox>
      </div>
    </div>

    <template #footer>
      <VustButton :disabled="store.containerCreateLoading" @click="store.cancelContainerCreate">
        {{ t('common.cancel') }}
      </VustButton>
      <VustButton
        type="primary"
        :loading="store.containerCreateLoading"
        :disabled="imageOptions.length === 0"
        @click="store.submitContainerConfig"
      >
        {{ t('app.docker.containers.createContainer') }}
      </VustButton>
    </template>
  </VustDialog>
</template>

<style scoped>
.create-form,
.advanced-fields,
.field-group {
  display: flex;
  flex-direction: column;
}

.create-form,
.advanced-fields {
  gap: var(--vdl-space-4);
}

.form-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--vdl-space-4);
}

.form-grid.three-columns {
  grid-template-columns: repeat(3, minmax(0, 1fr));
}

.advanced-fields {
  padding-top: var(--vdl-space-3);
  border-top: 1px solid var(--vdl-border-subtle);
}

.field-group {
  gap: var(--vdl-space-2);
}

.group-heading {
  display: flex;
  align-items: center;
  justify-content: space-between;
  color: var(--vdl-text-secondary);
  font-size: var(--vdl-font-body-sm);
  font-weight: var(--vdl-font-weight-semibold);
}

.repeat-row {
  display: grid;
  align-items: center;
  gap: var(--vdl-space-2);
}

.repeat-row.two-columns {
  grid-template-columns: minmax(140px, 0.8fr) minmax(180px, 1.2fr) auto;
}

.repeat-row.port-columns {
  grid-template-columns: 1fr 1fr 110px auto;
}

.repeat-row.mount-columns {
  grid-template-columns: 110px 1fr 1fr auto auto;
}

@media (max-width: 760px) {
  .form-grid,
  .form-grid.three-columns,
  .repeat-row.two-columns,
  .repeat-row.port-columns,
  .repeat-row.mount-columns {
    grid-template-columns: 1fr;
  }
}
</style>
