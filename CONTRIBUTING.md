# Contributing

Read the [architecture](docs/architecture.md) and [development guide](docs/development.md)
before making a substantial change. Coding agents should read [AGENTS.md](AGENTS.md).

## Issues and changes

For a bug report, include the Kyler version, operating system, reproduction steps,
expected behavior, and relevant redacted errors. Use [SECURITY.md](SECURITY.md) for
security issues rather than publishing exploit details or credentials.

For a feature proposal, describe the user problem, intended behavior, and affected
interfaces. Keep pull requests focused. Discuss persistence, protocol, permission,
and installation changes before broad rewrites.

## Development workflow

1. Work on a dedicated branch for your change.
2. Follow nearby naming, import grouping, and formatting conventions.
3. Add or update meaningful tests for changed behavior.
4. Run checks for the affected crates or frontend components.
5. Update documentation when commands, configuration, or behavior change.
6. Review the diff for generated files, private data, and unrelated edits.

Keep lockfiles with intentional dependency changes. Do not commit build caches,
`dist/`, local configuration, keys, private benchmark data, or evaluation results.
There is currently no repository CI workflow; record the local checks you ran.

## Commits and pull requests

Use concise, descriptive commits, for example `fix: restore hidden installer input`.
Existing history commonly uses `feat:`, `fix:`, and similar prefixes.

A pull request should explain the concrete problem, resulting behavior, and
validation. Describe compatibility or migration requirements when applicable.
Distinguish tested behavior from assumptions; do not claim live-provider or
cross-platform testing that was not performed.

## Licensing

Kyler is licensed under the [MIT License](LICENSE). Contributions are made under
the same license unless explicitly agreed otherwise. Preserve project and
third-party copyright and license notices.
