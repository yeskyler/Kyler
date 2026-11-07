<div align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo-dark.svg">
    <img src="docs/assets/logo-light.svg" width="328" height="104" alt="Kyler">
  </picture>
  <h3>A general-purpose AI assistant for research, creation, and getting things done.</h3>
  <p>
    <a href="https://github.com/yeskyler/Kyler/releases"><img src="https://img.shields.io/github/v/release/yeskyler/Kyler?style=flat-square&amp;color=3b82f6" alt="Release"></a>
    <a href="https://github.com/yeskyler/Kyler/actions/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflow/status/yeskyler/Kyler/release.yml?style=flat-square&amp;label=release%20build" alt="Release build"></a>
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
  <p><strong>English</strong> · <a href="README.zh-CN.md">简体中文</a></p>
  <p><a href="#installation">Install</a> · <a href="#capabilities">Capabilities</a> · <a href="#documentation">Documentation</a></p>
</div>

Kyler is a general-purpose AI assistant for research, writing, analysis, and software
development. It combines conversations, tools, and task deliverables in one workspace,
with Web, TUI, and CLI interfaces powered by a shared Rust runtime. Models connect
through OpenAI-compatible Chat Completions endpoints.

<p align="center">
  <img src="docs/assets/web-preview.png" width="1120" alt="Kyler Web interface">
</p>

## Capabilities

- Streaming responses, structured tool activity, and persistent conversations.
- File operations, managed commands, sandbox permissions, and execution approvals.
- MCP connectors and system, user, and project skills.
- Image and document attachments, downloadable deliverables, and artifact previews.
- Context management, project memory, and session inspection.
- Asynchronous subagents and independent read-only code reviews.

## Installation

Install from [GitHub Releases](https://github.com/yeskyler/Kyler/releases).
The complete package includes the backend, Web application, and Node.js runtime.

Download and run the installer:

```sh
curl -fL https://github.com/yeskyler/Kyler/releases/latest/download/install.sh -o install.sh
sh install.sh --modify-path
```

Follow setup to configure the model, API URL, API key, and service addresses.
The backend and Web service start automatically. Open **http://127.0.0.1:30002**.

```sh
kyler service status
kyler service stop
kyler service start
```

## Development

Install stable [Rust](https://rust-lang.org/tools/install/) and
[Node.js 24](https://nodejs.org/en/download) with npm.

```sh
git clone https://github.com/yeskyler/Kyler.git
cd Kyler
cargo build --manifest-path kyler-rs/Cargo.toml -p kyler-cli
```

Configure and start the backend:

```sh
export KYLER_CONFIG="$HOME/.kyler-dev/config.toml"
./kyler-rs/target/debug/kyler configure --addresses
./kyler-rs/target/debug/kyler serve
```

Start Web in a second terminal from the repository root:

```sh
export KYLER_CONFIG="$HOME/.kyler-dev/config.toml"
cd web
npm ci
npm run dev -- --hostname 127.0.0.1 --port 30002
```

Open **http://127.0.0.1:30002**. Development and installed services must use separate
ports when running together.

## Documentation

| Guide | Contents |
| --- | --- |
| [Installation](docs/installation.md) | Setup, service management, and upgrades |
| [Configuration](docs/configuration.md) | Models, paths, and application settings |
| [Architecture](docs/architecture.md) | Runtime, storage, and execution boundaries |
| [Development](docs/development.md) | Build requirements and validation |
| [Releasing](docs/releasing.md) | Packaging and automated publishing |
| [Troubleshooting](docs/troubleshooting.md) | Installation and runtime issues |
| [Evaluations](docs/evaluations.md) | GAIA and startup performance |

## License

Kyler is released under the [MIT License](LICENSE).
Third-party components retain their respective licenses and notices.

## Star history

<a href="https://www.star-history.com/#yeskyler/Kyler&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=yeskyler/Kyler&type=Date&theme=dark">
    <img src="https://api.star-history.com/svg?repos=yeskyler/Kyler&type=Date" alt="Kyler GitHub star history" width="800">
  </picture>
</a>

[View the interactive chart](https://www.star-history.com/#yeskyler/Kyler&Date).
