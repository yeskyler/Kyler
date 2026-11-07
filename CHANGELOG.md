# Changelog

Changes below describe implemented work that has not been assigned a published
release in this document. Package version `0.1.0` alone does not establish a
publication date. Earlier development history remains available in Git.

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
