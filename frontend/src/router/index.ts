import { createRouter, createWebHistory } from 'vue-router'

import LoginView from '../views/LoginView.vue'
import DesktopLayout from '../components/layout/DesktopLayout.vue'
import DesktopView from '../views/DesktopView.vue'
import http from '@/api'
import type { AuthBody } from '@/api/interface'
import { getLoginEntryPath, rememberLoginEntry } from '@/utils/login-entry'

const router = createRouter({
  history: createWebHistory('/'),
  routes: [
    {
      path: '/login',
      name: 'login',
      component: LoginView,
    },
    {
      path: '/:safeEntry([A-Za-z0-9]{8,32})',
      name: 'safe-login',
      component: LoginView,
    },
    {
      path: '/upgrade-progress',
      name: 'upgrade-progress',
      component: () => import('../apps/views/UpgradeProgressView.vue'),
    },
    {
      path: '/',
      component: DesktopLayout,
      children: [
        {
          path: '', // 映射到 /
          name: 'desktop',
          component: DesktopView,
          meta: { requiresAuth: true },
        },
      ],
    },
    // 捕获所有未匹配路径，跳转到首页
    {
      path: '/:pathMatch(.*)*',
      redirect: '/',
    },
  ],
})

let isLoggedIn = false
let hasChecked = false

/**
 * @description 路由守卫：处理登录拦截逻辑
 */
router.beforeEach(async (to, from) => {
  const fromLogin = from.path === '/login' || from.name === 'safe-login'
  const toLogin = to.path === '/login' || to.name === 'safe-login'

  if (to.name === 'safe-login') {
    rememberLoginEntry(to.path)
  }

  // 从登录页去往非登录页时，只有在尚未明确标记登录成功的情况下才重新请求 session。
  if (fromLogin && !toLogin && !isLoggedIn) {
    hasChecked = false
  }

  // 1. 如果去往需要授权的页面
  if (to.meta.requiresAuth) {
    if (!hasChecked) {
      const res = await http.get<AuthBody>('/auth/me')
      isLoggedIn = res.success
      hasChecked = true
    }
    if (!isLoggedIn) {
      return getLoginEntryPath()
    }
  }

  // 2. 如果去往登录页
  if (toLogin) {
    if (!hasChecked) {
      const res = await http.get<AuthBody>('/auth/me')
      isLoggedIn = res.success
      hasChecked = true
    }
    if (isLoggedIn) {
      return '/'
    }
  }
})

export function resetAuthState() {
  isLoggedIn = false
  hasChecked = false
}

export function markAuthState(authenticated: boolean) {
  isLoggedIn = authenticated
  hasChecked = true
}

export default router
