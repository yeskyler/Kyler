<div align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo-dark.svg">
    <img src="docs/assets/logo-light.svg" width="328" height="104" alt="Kyler">
  </picture>
  <h3>面向研究、创作与任务执行的通用 AI 助手。</h3>
  <p>
    <a href="https://github.com/yeskyler/Kyler/releases"><img src="https://img.shields.io/github/v/release/yeskyler/Kyler?style=flat-square&amp;color=3b82f6" alt="Release"></a>
    <a href="LICENSE"><img src="https://img.shields.io/github/license/yeskyler/Kyler?style=flat-square&amp;color=22c55e" alt="License"></a>
    <a href="https://github.com/yeskyler/Kyler/stargazers"><img src="https://img.shields.io/github/stars/yeskyler/Kyler?style=flat-square&amp;color=eab308" alt="GitHub stars"></a>
  </p>
  <p>
    <a href="docs/development.md"><img src="https://img.shields.io/badge/runtime-Rust-dea584?style=flat-square&amp;logo=rust&amp;logoColor=white" alt="Rust runtime"></a>
    <a href="docs/architecture.md"><img src="https://img.shields.io/badge/interfaces-Web%20%2F%20TUI%20%2F%20CLI-475569?style=flat-square" alt="Web, TUI, and CLI"></a>
    <a href="docs/architecture.md"><img src="https://img.shields.io/badge/sandbox-MXC-0078d4?style=flat-square" alt="MXC Sandbox"></a>
    <a href="docs/configuration.md"><img src="https://img.shields.io/badge/connectors-MCP-6366f1?style=flat-square" alt="MCP connectors"></a>
    <a href="docs/configuration.md"><img src="https://img.shields.io/badge/extensions-Skills-10b981?style=flat-square" alt="Skills"></a>
  </p>
  <p><a href="README.md">English</a> · <strong>简体中文</strong></p>
  <p><a href="#安装">安装</a> · <a href="#主要能力">主要能力</a> · <a href="#文档导航">文档导航</a></p>
</div>

Kyler 是面向研究、写作、分析和软件开发的通用 AI 助手，将对话、工具与任务产物
整合在同一个工作空间中。Web、TUI 和 CLI 共用 Rust 运行时，通过兼容
OpenAI Chat Completions 的接口连接模型服务。

<p align="center">
  <img src="docs/assets/web-preview.png" width="1120" alt="Kyler Web 界面">
</p>

## 主要能力

- 流式回复、结构化工具活动和持久化会话。
- 文件操作、进程管理、沙箱权限和执行审批。
- MCP 连接器，以及系统级、用户级和项目级技能。
- 图片与文档附件、可下载产物和产物预览。
- 上下文管理、项目记忆和会话状态查看。
- 异步子智能体和独立的只读代码审查。

## 安装

通过 [GitHub Releases](https://github.com/yeskyler/Kyler/releases) 安装。
完整发行包包含后端、Web 前端和 Node.js 运行时。

下载并运行安装程序：

```sh
curl -fL https://github.com/yeskyler/Kyler/releases/latest/download/install.sh -o install.sh
sh install.sh
```

按提示配置模型、API 地址、密钥和服务地址，前后端服务会自动启动。
在浏览器打开 **http://127.0.0.1:30002**。

```sh
kyler service status
kyler service stop
kyler service start
```

## 开发

在 macOS 或 Linux 上安装 stable [Rust](https://rust-lang.org/tools/install/) 和
[Node.js 24](https://nodejs.org/en/download)，Node.js 自带 npm。

```sh
git clone https://github.com/yeskyler/Kyler.git
cd Kyler
```

首次使用时配置后端，然后启动；`cargo run` 会自动完成所需的编译：

```sh
cargo run --manifest-path kyler-rs/Cargo.toml -p kyler-cli -- configure --addresses
cargo run --manifest-path kyler-rs/Cargo.toml -p kyler-cli -- serve
```

前后端默认使用 `~/.kyler/config.toml`。已有有效配置时，可以跳过 `configure`。
在另一个终端中，从仓库根目录启动 Web：

```sh
cd web
npm ci
npm run dev -- --hostname 127.0.0.1 --port 30002
```

打开 **http://127.0.0.1:30002**。开发服务与已安装的服务同时运行时，应使用不同端口。

## 文档导航

| 文档 | 内容 |
| --- | --- |
| [安装](docs/installation.md) | 配置、服务管理和升级 |
| [配置](docs/configuration.md) | 模型、路径和应用设置 |
| [架构](docs/architecture.md) | 运行时、存储与执行边界 |
| [开发](docs/development.md) | 构建环境和验证 |
| [发布](docs/releasing.md) | 打包和自动发布 |
| [排错](docs/troubleshooting.md) | 安装和运行问题 |
| [评测](docs/evaluations.md) | GAIA 与启动性能 |

## 开源协议

Kyler 采用 [MIT License](LICENSE) 开源。
第三方组件保留各自的许可证和声明。

## 仓库人气记录

<a href="https://www.star-history.com/#yeskyler/Kyler&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=yeskyler/Kyler&type=Date&theme=dark">
    <img src="https://api.star-history.com/svg?repos=yeskyler/Kyler&type=Date" alt="Kyler GitHub Star 历史记录" width="800">
  </picture>
</a>

[查看交互图表](https://www.star-history.com/#yeskyler/Kyler&Date)。
