<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { versionStaticAsset } from '@/utils/static-assets'
import VustIcon from './VustIcon.vue'

const props = withDefaults(
  defineProps<{
    name: string
    size?: number
    label?: string
    decorative?: boolean
  }>(),
  {
    size: 24,
    label: '',
    decorative: true,
  },
)

const isImageIcon = computed(() => {
  const value = props.name.trim()
  return value.startsWith('/') || value.startsWith('http://') || value.startsWith('https://')
})

const fallbackImage = versionStaticAsset('/images/apps/fallback.png')
const imageFallbackLevel = ref(0)
const imageSource = computed(() => (imageFallbackLevel.value === 0 ? props.name : fallbackImage))
const showImage = computed(() => isImageIcon.value && imageFallbackLevel.value < 2)

watch(
  () => props.name,
  () => {
    imageFallbackLevel.value = 0
  },
)

/**
 * @description 图片图标加载失败时先使用应用 PNG 兜底，再回退到 SVG 兜底图标。
 */
function handleImageError() {
  imageFallbackLevel.value += 1
}
</script>

<template>
  <img
    v-if="showImage"
    class="app-icon-image"
    :src="imageSource"
    :width="size"
    :height="size"
    :alt="decorative ? '' : label"
    :aria-hidden="decorative ? 'true' : undefined"
    draggable="false"
    @error="handleImageError"
  />
  <VustIcon
    v-else
    :name="isImageIcon ? 'fallback' : name"
    :size="size"
    :label="label"
    :decorative="decorative"
  />
</template>

<style scoped>
.app-icon-image {
  display: inline-block;
  flex: 0 0 auto;
  object-fit: contain;
  color: currentColor;
}
</style>
