# Agent instructions

## Scope and layout

These instructions apply to the repository. Read relevant code and documentation
before changing behavior; user instructions take precedence over this file.

- `kyler-rs/Cargo.toml` defines the Rust workspace; crates live in `kyler-rs/crates/`.
- `web/` contains the Next.js frontend and its npm lockfile.
- `config/default.toml` is a template, not a user's configuration.
- `scripts/` owns installation and packaging; `docs/` owns detailed guides.
- The root `README.md` is the project entry point.

## Code conventions

- Use English for identifiers, comments, error messages, and canonical documentation.
  Keep translations in explicitly named files such as `README.zh-CN.md`.
- Match existing naming and formatting. Rust files use `snake_case`; frontend
  component files generally use `kebab-case`.
- Rust import groups are local modules, standard library, external dependencies,
  then workspace crates. Separate groups with blank lines and follow nearby code.
- Keep related small types and implementations together. Split files when their
  responsibilities justify it; avoid a file or folder for every implementation.
- Keep changes focused; avoid unrelated refactoring.

## Architecture boundaries

- Keep execution in `kyler-core`, HTTP/SSE in `kyler-server`, shared contracts in
  `kyler-protocol`, and client transport in `kyler-client`.
- Maintain matching structured event behavior in Web and TUI when changing events.
- Sandbox permissions and tool approval are separate controls. Client selections
  cannot grant authority beyond host policy.
- Keep credentials server-side. Never expose keys in browser payloads, logs, errors,
  fixtures, or documentation examples.
- Product system prompts live in `kyler-protocol/src/prompts/`; this file describes
  repository development, not the installed assistant's system prompt.

## Validation

Run commands from the repository root unless noted otherwise:

```sh
cargo fmt --manifest-path kyler-rs/Cargo.toml --all -- --check
cargo check --manifest-path kyler-rs/Cargo.toml --workspace
cargo test --manifest-path kyler-rs/Cargo.toml -p <affected-crate>
```

From `web/`, use `npm run lint`, `npm test`, and `npm run build` as appropriate.
For installer changes, build `kyler-cli`, then run
`python3 scripts/tests/test_install.py`. See `docs/development.md` for details.
Documentation-only changes need link/path checks, not a full application rebuild.
Run meaningful checks and report failures or checks not run.

## Working data and completion

- Preserve existing changes and local user data.
- Keep caches, `dist/`, secrets, private datasets, and evaluation results out of
  commits. Review `.gitignore` after moving directories.
- Test installation with a temporary HOME and ports, not the developer's installed
  configuration or running services.
- Do not run live benchmarks or paid model requests unless the user authorizes them.
- Commit, push, or publish only when requested by the user.
- Summarize changes, relevant checks, and unresolved limitations when finishing.
