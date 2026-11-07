# Development

Run commands from the repository root unless a different directory is specified.
The Rust workspace is inside `kyler-rs/`; the repository root has no Cargo manifest.

## Requirements

- The current stable Rust toolchain from [rustup](https://rust-lang.org/tools/install/),
  including Cargo, rustfmt, and support for Rust 2024.
- [Node.js 24](https://nodejs.org/en/download) and npm for Web development; release
  packaging uses the same native Node.js major version.
- Python 3.9+ for packaging and installer tests.
- macOS or Linux for the native bundle/service workflow. Native sandbox behavior
  must be checked on the target platform; do not infer cross-platform coverage.

On macOS, install command-line build tools with `xcode-select --install`.
On Ubuntu/Debian, the usual starting dependencies are `build-essential`, `pkg-config`,
and `libssl-dev`; native Linux sandbox tests also need `bubblewrap` and a host
that permits its user namespaces. Install any additional native libraries reported
by the build.
Install Python through your operating system or the official Python distribution.
Verify the development tools before building:

```sh
rustc --version
cargo --version
node --version
npm --version
python3 --version
```

Complete Release users do not need these tools; see [installation](installation.md).

## Rust

```sh
cargo check --manifest-path kyler-rs/Cargo.toml --workspace
cargo build --manifest-path kyler-rs/Cargo.toml -p kyler-cli
cargo fmt --manifest-path kyler-rs/Cargo.toml --all -- --check
cargo test --manifest-path kyler-rs/Cargo.toml -p kyler-config
cargo test --manifest-path kyler-rs/Cargo.toml -p kyler-cli --test service
```

Targeted crate tests are normally sufficient for local changes. For a broad
integration change, run the relevant additional suites or the workspace suite:

```sh
cargo test --manifest-path kyler-rs/Cargo.toml --workspace
```

Some tests spawn local HTTP servers, subprocesses, PTYs, or native sandboxes.
Restricted execution environments may need permission to run those checks.
Use `--offline` only when the needed dependencies are already cached.

## Web

With a configured backend running, use another terminal and the same configuration:

```sh
export KYLER_CONFIG="$HOME/.kyler-dev/config.toml"
cd web
npm ci
npm run dev -- --hostname 127.0.0.1 --port 30002
```

From `web/`, validate frontend work with:

```sh
npm run lint
npm test
npm run build
```

The frontend reads the backend connection from server-side configuration. If
port 30002 is occupied by the installed Web service, use another development port.
Do not run destructive tests against a normal user's workspace or session store.

## Configuration and local execution

For first-time backend setup from a checkout:

```sh
export KYLER_CONFIG="$HOME/.kyler-dev/config.toml"
cargo run --manifest-path kyler-rs/Cargo.toml -p kyler-cli -- configure --addresses
cargo run --manifest-path kyler-rs/Cargo.toml -p kyler-cli -- serve
```

Setup writes user configuration; use explicit `--config` with a separate setup file
when you need an isolated developer configuration. See
[configuration](configuration.md) for path and override rules.

## Installer and release checks

Build the CLI before running installer tests so integration cases are not skipped:

```sh
cargo build --manifest-path kyler-rs/Cargo.toml -p kyler-cli
python3 scripts/tests/test_install.py
python3 scripts/tests/test_release.py
sh -n scripts/install.sh scripts/package.sh scripts/release.sh
```

The installer tests use temporary homes and ports. Node.js is needed for the
complete-bundle fixture tests. Those fixtures test installation behavior; a real
standalone Web smoke check is also useful before publishing a release.

See [releasing](releasing.md) for producing a native package. Documentation changes
should be checked for broken links, stale paths, and commands inconsistent with
current `--help`; they do not require a full Rust/frontend rebuild.

## Working files

Rust output belongs in `kyler-rs/target/`, Next.js output in `web/.next/`, and
release output in `dist/`. Private GAIA datasets and local reports stay in the
ignored directories under `kyler-rs/crates/kyler-evals/`. Preserve local data when
moving or cleaning build directories.
