# Changelog

## 0.6.0 — 2026-11-21

### Added

- Dedicated provider adapters and a session gateway with scoped tokens and request auditing.
- Native CLI installation with a cliclack setup wizard and bundled ripgrep.
- Skill proposal review cards in assistant messages.

### Changed

- Linux sandbox execution uses system Bubblewrap and network dependencies, with pre-install validation.
- Model discovery and model-aware provider request projection.
- Chat model selection resets thinking to Off; all seven grades remain manually selectable.
- System guidance reflects the execution loop, progressive tool discovery and reviewed experience.

### Fixed

- Provider failures retain actionable diagnostics in the Web interface.
- Refined plugin menus and centered sidebar sign-out control.

## 0.5.0 — 2026-11-19

### Added

- Dedicated MCP, Skills, SQLite storage and ACP integration crates.
- Explicit BM25 tool retrieval and progressive skill loading.
- Reviewed memory candidates, source-bound skill drafts and Memory/Skills review interfaces.
- English runtime-centered architecture diagrams and bilingual Web screenshots.

### Fixed

- The Web home page starts a new conversation instead of reopening the recent session.

## 0.2.0 — 2026-11-18

### Added

- SQLite-backed automations, session indexes, and shared Web navigation preferences.
- Product-style plugin management and chart-based session inspection panels.
- Configurable model context windows and bounded parallel execution of compatible tools.

### Changed

- Session execution uses `cwd`, with project identity and managed-directory ownership
  separated from host sandbox authority.
- Web, TUI, CLI, automations, and artifact paths use the canonical directory contract.
- Application database schema v3 removes obsolete directory index columns.

### Fixed

- Upgrade Next.js to 16.4.5 to incorporate the November security fixes.
- Isolated-session permission coverage now verifies confirmed Full access selection
  and preserves the selected execution directory.
- Session creation forwarding accepts `cwd`; deletion preserves unmanaged project files.

### Upgrade notes

- Back up application data before upgrading. Removed session environment fields and
  old automation directory aliases are not accepted. Export or migrate old-format
  histories and task definitions before use; upgrading does not delete them.
- Database indexes upgrade automatically from schema v1/v2 to v3. Older binaries
  cannot open the upgraded index. See [session persistence](docs/session-persistence.md).

## Unreleased

### Added

- Complete native bundles containing Kyler, the standalone Web UI, and Node.js.
- Configurable backend and Web addresses, Web access authentication, and unified
  background service management.
- GAIA evaluation tooling and isolated startup performance measurements.
- Delegated read-only review agents and persistent agent names with task limits.
- Root project guidance and guides for installation, architecture, configuration,
  development, release packaging, troubleshooting, and evaluations.

### Changed

- Rust workspace and build output reside in `kyler-rs/`; frontend source resides
  in `web/`. Installation scripts and configuration templates remain at the root.
- Default backend and Web addresses use ports `30001` and `30002`.
- User installation data remains under `~/.kyler/` independently of source layout.

### Fixed

- Hidden installer input on macOS no longer depends on terminal event-reader
  initialization. Normal terminal, controlling-terminal, and piped installer
  input paths have regression coverage.
- Complete-bundle upgrades restore the previous installation and configuration
  when service startup fails and recovery can complete safely.
