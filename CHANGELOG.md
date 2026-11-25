# Changelog

## 0.0.2 — Unreleased

### Added

- QQ Bot WebSocket delivery with heartbeat monitoring, reconnect/resume, and private-message pairing.
- Independent provider model selection and defaults in Web Settings and CLI setup.
- Automatic thinking-level fallback when model capabilities are unavailable.

### Fixed

- Keep hosted model aliases out of native DeepSeek connections after provider changes.
- Simplify provider labels and align selection indicators in the chat model menu.

## 0.0.1

The public release series starts at 0.0.1.

### Added

- CLI and local terminal assistant, with configurable model providers and tool execution.
- Web interface for conversations, provider settings, skills, memory and delegated tasks.
- Session persistence, scheduled tasks, messaging channels and gateway access controls.
- Native macOS and Linux bundles with Node.js and ripgrep, installed by the shell script.
- Model discovery and separate provider connection and model-list refresh states.

### Release notes

- Agent execution defaults to a maximum of 100 steps.
- Configure credentials with `kyler configure` and start services with `kyler service start`.
- Run `kyler doctor` to check native sandbox support without model requests.
