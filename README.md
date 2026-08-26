# CleanerX

CleanerX 是一个本地优先的 coding agent 存储清理器。MVP 使用 Rust、Tauri 2 和 React 构建，专注于 macOS 13+ 上的 Codex 本地数据。

它不会连接云端、上传数据或收集遥测。项目仅用于聚合 Codex 数据；CleanerX 不递归扫描项目目录，也不会删除源码。

## MVP 能力

- 通过 Codex App Server 分页读取当前、归档和子会话，并通过官方 `thread/delete` 删除会话树。
- 按规范化项目根聚合会话，但从不修改项目目录或 Codex 项目注册信息。
- 会话是项目数据的唯一入口，默认按“项目根 → 根会话 → 分支/子会话”树形展示，并保留可筛选的列表视图；不再设置功能重叠的独立项目页。
- 会话、记忆、附件、生成内容、日志、缓存和临时文件均可打开详情；正文只在用户主动打开单项详情时按需、只读、限量加载，不进入扫描快照。
- 统计会话 rollout、附件、生成图片、可视化、日志、缓存与临时文件的实际磁盘占用。
- 所有清理项均不默认选择，用户可在当前筛选范围内手动全选、取消全选或逐项选择；固定和活动会话仍不可清理。
- 重要数据先写入加密 `.cxb` 备份，验证完成后才会开始清理。
- 认证、配置、规则、技能、插件与源码始终受保护。
- 支持系统中英文、深浅色、键盘导航和减少动态效果。

## 安全模型

会话写操作优先且仅通过 Codex App Server 完成。CleanerX 不使用私有 SQLite 写入作为删除兜底。无法建立 App Server 连接或无法使用 `thread/delete` 时，会话清理自动降级为只读报告；`memory/reset` 不可用时只禁用独立的记忆重置操作。

直接文件清理受固定根目录和类别白名单限制，并拒绝符号链接、路径逃逸、受保护路径和操作前身份变化。日志数据库只有在 schema 明确匹配时才使用事务、WAL checkpoint 与 `VACUUM` 清理。

`.cxb` 格式为 tar + zstd + [age](https://age-encryption.org/) X25519 加密。私钥存储在 macOS Keychain；归档先写 `.partial`，完成校验后原子改名。恢复绝不覆盖已存在的同路径数据。

更完整的说明见 [文档索引](docs/README.md)、[后续开发计划](docs/roadmap.md)、[存储模型](docs/storage-model.md)、[跨 Agent 会话层级调研](docs/agent-session-hierarchy.md)、[开发代理约束](AGENTS.md) 与 [SECURITY.md](SECURITY.md)。

## 开发

要求：Rust 1.88+、Node.js 22+、pnpm 11+、macOS 上的 Tauri 系统依赖。

```bash
make setup
make dev
```

提交前执行与 CI 对齐的质量流水线：

```bash
make check
```

它依次检查 Rust 格式、Clippy、Rust/前端测试和前端生产构建。底层仍直接使用 Cargo、pnpm 与 Tauri；Makefile 只是稳定的本地和 CI 编排入口。

日常调试可只构建未签名 `.app`，无需生成或挂载 DMG；发布检查可一次生成 `.app` 和 DMG：

```bash
make app
open target/release/bundle/macos/CleanerX.app

make bundles
```

开发期间需要热更新和前后端日志时，使用 `make dev`。

Apple Silicon 和 Intel 产物分别在对应架构 runner 上构建。未签名应用首次打开时，macOS 可能阻止运行；在 Finder 中右键 CleanerX →“打开”，或前往“系统设置 → 隐私与安全性”确认打开。不要对来历不明的二进制绕过 Gatekeeper。

## 只读模式排障

CleanerX 会先尝试连接正在运行的 Codex 控制 socket；若 socket 已残留或没有响应，会自动回退到隔离的临时 stdio App Server，不需要强制退出 Codex。Finder 启动时即使 PATH 不完整，也会探测 Codex/ChatGPT 应用内置二进制以及常见 Homebrew、NVM、Volta、asdf、Bun 和 pnpm 安装位置。

如果界面仍显示只读报告：

1. 点击警告中的“重试连接”，并查看警告下方的具体原因。
2. 确认已安装 Codex CLI 或 ChatGPT/Codex 桌面应用；终端执行 `codex --version` 和 `codex app-server --help` 应成功。
3. 如果使用自定义目录，在“设置”中填写绝对 `CODEX_HOME` 路径后重新扫描。
4. 升级 Codex 后重启 CleanerX。CleanerX 不会通过修改私有 SQLite 绕过缺失的官方删除能力。

## 架构

```text
crates/cleanerx-core   domain types, planning, path safety, .cxb backup/restore
crates/adapter-codex  installation detection, App Server client, capability-aware scan
src-tauri             narrow command boundary and cleanup transaction orchestration
src                   React/TypeScript GUI
```

未来 Claude Code、OpenCode 与 Pi 适配器通过编译期 `AgentAdapter` trait 接入。MVP 不加载动态插件，也不授予前端通用文件系统或 Shell 能力。

## Official Codex interfaces

CleanerX follows the public [Codex App Server protocol](https://learn.chatgpt.com/docs/app-server) for session operations and treats [Codex memory](https://learn.chatgpt.com/docs/customization/memories) as global data. Protocol capabilities are negotiated at runtime because Codex evolves independently of CleanerX.

## License

Apache-2.0. See [LICENSE](LICENSE).
