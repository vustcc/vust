import { fileURLToPath, URL } from 'node:url'
import { execSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { DEFAULT_CONTROLLER_PORT } from './src/utils/constants'

import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import vueDevTools from 'vite-plugin-vue-devtools'
import Components from 'unplugin-vue-components/vite'

const packageJson = JSON.parse(
  readFileSync(new URL('./package.json', import.meta.url), 'utf-8'),
) as {
  version?: string
}
const appVersion = packageJson.version ?? '0.0.0'

function getGitHash(): string {
  try {
    return execSync('git rev-parse --short=12 HEAD', { encoding: 'utf-8' }).trim()
  } catch {
    return 'unknown'
  }
}

const gitHash = getGitHash()

// https://vite.dev/config/
export default defineConfig(({ command }) => {
  const isBuild = command === 'build'

  return {
    define: {
      'import.meta.env.VITE_APP_VERSION': JSON.stringify(appVersion),
      'import.meta.env.VITE_GIT_HASH': JSON.stringify(gitHash),
    },
    plugins: [
      vue(),
      vueDevTools(),
      Components({
        dts: isBuild ? false : 'src/components.d.ts',
        dirs: ['src/components'],
        extensions: ['vue'],
        include: [/\.vue$/, /\.vue\?vue/],
        resolvers: [],
      }),
    ],
    resolve: {
      alias: {
        '@': fileURLToPath(new URL('./src', import.meta.url)),
      },
    },
    server: {
      // 配置代理，解决开发环境下的跨域问题
      proxy: {
        // 当有 /api 前缀 of 请求时，转发到后端的默认端口
        '/api': {
          target: `https://127.0.0.1:${DEFAULT_CONTROLLER_PORT}`,
          changeOrigin: true,
          secure: false,
          ws: true,
        },
      },
    },
    build: {
      rolldownOptions: {
        output: {
          strictExecutionOrder: true,
          codeSplitting: {
            groups: [
              {
                name: 'vendor-vue',
                test: /node_modules[\\/](vue|vue-router|vue-i18n|pinia|pinia-plugin-persistedstate)[\\/]/,
                maxSize: 250 * 1024,
                priority: 40,
              },
              {
                name: 'vendor-monaco',
                test: /node_modules[\\/]monaco-editor[\\/]/,
                maxSize: 500 * 1024,
                priority: 30,
              },
              {
                name: 'vendor-xterm',
                test: /node_modules[\\/]@xterm[\\/]/,
                maxSize: 250 * 1024,
                priority: 30,
              },
              {
                name: 'vendor-echarts',
                test: /node_modules[\\/]echarts[\\/]/,
                maxSize: 500 * 1024,
                priority: 30,
              },
            ],
          },
        },
      },
    },
  }
})
