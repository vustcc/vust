# Changelog

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，并遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### Added

- 登录页新增 GitHub 项目入口、浅色与深色主题切换，以及中文与英文语言选择。
- 新增 Liquid Glass 外观，覆盖桌面、顶部栏、应用库、窗口、登录页与右键菜单，并支持在外观设置中开关和持久化。
- 套件主题状态新增液态玻璃同步能力，使主控与新版套件保持一致的材质表现。

### Changed

- 升级 VUST UI、Tokens、Icons 与 Suite SDK 至 `0.1.0-alpha.2`，统一组件、滚动条和框架表面的公共玻璃材质。
- 优化窗口玻璃层的合成方式，保持终端、代码编辑器和应用内容清晰可读。
- Docker 的项目、容器、镜像、数据卷和网络创建流程改为在应用内容区完成，保留列表状态、后台部署进度与上传中止能力，并统一创建操作术语和键盘焦点行为。

## [0.1.0-alpha.1](https://github.com/vustcc/vust/commits/0.1.0-alpha.1) - 2026-09-10

### Added

- 首次发布 VUST 主控服务、分布式节点 Agent 与 `vustctl` 命令行工具。
- 提供统一管理一台或多台 Linux 主机的 Web 控制台与主机运维能力。
- 支持通过 `.vsp` 套件包按需扩展平台能力。
- 提供 Docker、文件、进程、磁盘、防火墙、终端、脚本、计划任务、日志与在线升级管理能力。
