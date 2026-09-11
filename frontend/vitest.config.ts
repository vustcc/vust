/**
 * @file vitest.config.ts
 * @description 前端组件与 composable 回归测试配置。
 */

import { fileURLToPath, URL } from 'node:url'
import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  plugins: [vue()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  test: {
    environment: 'happy-dom',
    clearMocks: true,
    restoreMocks: true,
  },
})
