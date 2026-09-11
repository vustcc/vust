# VUST 控制台 (frontend)

## 简介

本项目是 VUST 的前端控制台用户界面（UI）。它基于 Vue 3 和 Vite 构建，提供桌面化的运维管理体验，用于 Docker 管理、文件操作与日志查看等功能。

## 🛠️ 技术栈

* **核心框架**：Vue 3.5.x
* **构建工具**：Vite 8.x
* **状态管理**：Pinia 3.0.x
* **路由**：Vue Router 5.x
* **语言**：TypeScript 6.x
* **代码规范**：ESLint、Prettier

## 🚀 项目设置与运行

### 环境要求

请确保你的 Node.js 版本符合要求：

```json
// package.json snippet
"engines": {
  "node": "^20.19.0 || >=22.12.0"
}
````

### 安装依赖

```sh
pnpm install
```

### 开发模式 (Dev Server)

运行开发服务器。Vite 配置了 `/api` 代理，会将所有 API 请求转发到后端服务 `http://127.0.0.1:7310`：

```sh
pnpm dev
```

### 生产构建 (Production Build)

编译、类型检查并压缩项目文件用于生产环境部署：

```sh
pnpm build
```

## ⚙️ 代码规范与维护

### 类型检查

在构建或开发前，推荐运行类型检查：

```sh
pnpm type-check
```

### 代码规范检查 (Linting)

本项目使用 ESLint 进行代码质量和潜在错误检查。

```sh
# 使用 ESLint 检查并修复问题
pnpm lint:eslint

# 运行所有 linting 任务
pnpm lint
```

### 格式化 (Formatting)

使用 Prettier 统一代码风格：

```sh
pnpm format
```

## 🌐 API 架构说明

前端通过 `/api` 路径与后端服务进行交互。在开发环境中，请求会被代理到后端：

```typescript
// vite.config.ts snippet
proxy: {
  '/api': {
    target: '[http://127.0.0.1:7310](http://127.0.0.1:7310)',
    changeOrigin: true,
  },
},
```

例如，登录请求会发送到 `/api/auth/login`。
