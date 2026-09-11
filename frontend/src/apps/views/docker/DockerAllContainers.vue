<script setup lang="ts">
/**
 * @file DockerAllContainers.vue
 * @description Docker 容器管理壳组件，负责在容器列表、创建向导与容器详情组件之间进行动态切换与路由。
 */

import { defineAsyncComponent, ref } from 'vue'
import { useDockerStore } from '@/stores/docker'
import type * as dockerType from '@/api/interface/docker'

// 声明原有属性以确保与主视图组件通信时的 TypeScript 类型兼容性
defineProps<{
  containers: dockerType.DockerContainerSummary[]
  networks?: dockerType.DockerNetworkSummary[]
  isCreateActive?: boolean
  containerStep?: 'selectImage' | 'config'
  selectedImageId?: string | null
  selectedImageLabel?: string
  containerForm?: {
    name: string
    command: string
    environment: Array<{ name: string; value: string }>
    ports: dockerType.DockerContainerCreatePort[]
    mounts: dockerType.DockerContainerCreateMount[]
    restartPolicy: dockerType.DockerContainerCreateRestartPolicy
    maximumRetryCount: number | null
    networkId: string
    autoRemove: boolean
  }
  containerResourceStats?: Record<
    string,
    {
      data: dockerType.ContainerResourceUsageSummary
      fetchedAt: number
    }
  >
  nodeId: string
}>()

defineEmits<{
  (
    e: 'action',
    id: string | null | (string | null)[],
    name: string | null | (string | null)[],
    action: dockerType.DockerContainerAction,
  ): void
  (e: 'update:containerStep', value: 'selectImage' | 'config'): void
  (e: 'update:selectedImageId', value: string | null): void
  (
    e: 'update:containerForm',
    value: {
      name: string
      command: string
      environment: Array<{ name: string; value: string }>
      ports: dockerType.DockerContainerCreatePort[]
      mounts: dockerType.DockerContainerCreateMount[]
      restartPolicy: dockerType.DockerContainerCreateRestartPolicy
      maximumRetryCount: number | null
      networkId: string
      autoRemove: boolean
    },
  ): void
  (e: 'cancelCreate'): void
  (e: 'submitCreate'): void
  (e: 'fetchContainerStats', id: string): void
  (e: 'update:detailActive', value: boolean): void
  (e: 'logsActiveChange', value: boolean): void
  (e: 'terminalActiveChange', value: boolean): void
  (e: 'create'): void
}>()

const store = useDockerStore()
const selectedContainerId = ref<string | null>(null)

// ─── 动态异步导入子组件 ───
const DockerContainerList = defineAsyncComponent(() => import('./DockerContainerList.vue'))
const DockerContainerCreateWizard = defineAsyncComponent(
  () => import('./DockerContainerCreateWizard.vue'),
)
const DockerContainerDetail = defineAsyncComponent(() => import('./DockerContainerDetail.vue'))
</script>

<template>
  <div class="docker-containers" data-page="docker-containers">
    <DockerContainerDetail
      v-if="store.isContainerDetailActive && selectedContainerId"
      :container-id="selectedContainerId"
      :node-id="nodeId"
      @back="
        () => {
          store.isContainerDetailActive = false
          selectedContainerId = null
        }
      "
      @logs-active-change="(value) => $emit('logsActiveChange', value)"
      @terminal-active-change="(value) => $emit('terminalActiveChange', value)"
    />
    <DockerContainerList
      v-else
      @open-detail="
        (id) => {
          selectedContainerId = id
          store.isContainerDetailActive = true
        }
      "
    />
    <DockerContainerCreateWizard v-if="store.isContainerCreateActive" />
  </div>
</template>

<style scoped>
.docker-containers {
  flex-grow: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
}
</style>
