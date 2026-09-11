<script setup lang="ts">
import { RouterView } from 'vue-router'
import DesktopHeader from './DesktopHeader.vue'

/**
 * @file DesktopLayout.vue
 * @description 平台的仿 macOS 桌面三段式布局组件。
 * 包含持久化的头部、可滚动的 RouterView 区域和持久化的底部坞。
 */
</script>

<template>
  <div class="desktop-layout">
    <DesktopHeader class="header-bar" />

    <div class="desktop-area" data-slot="content">
      <RouterView />
    </div>
  </div>
</template>

<style scoped>
.desktop-layout {
  display: flex;
  flex-direction: column;
  height: 100vh;
  background-color: var(--vdl-bg-canvas);
  /* --- 背景图设置 --- */
  background-image: var(--vdl-desktop-wallpaper);
  background-repeat: no-repeat;
  background-size: cover;
  background-position: center center;
  background-attachment: fixed;
  /* -------------------------- */
}

.desktop-area {
  /* 中部占据剩余空间，实现路由切换 */
  flex-grow: 1;
  padding: 0;
  margin: 0;
  position: relative;
}

/* 头部和底部的样式由各自的组件定义，这里只定义容器定位 */
.header-bar {
  flex-shrink: 0;
}
</style>
