# Development

Run commands from the repository root unless a different directory is specified. The
Rust workspace is inside `kyler-rs/`; the repository root has no Cargo manifest.

## Requirements

- The current stable Rust toolchain from [rustup](https://rust-lang.org/tools/install/),
  including Cargo, rustfmt, and support for Rust 2024.
- [Node.js 24](https://nodejs.org/en/download) and npm for Web development; release
  packaging uses the same native Node.js major version.
- Python 3.9+ for packaging and installer tests.
- macOS or Linux for native packaging and service execution. Native sandbox
  validation must run on each supported target platform.

On macOS, install command-line build tools with `xcode-select --install`. Ubuntu and
Debian builds require `build-essential`, `pkg-config`, and `libssl-dev`; native Linux
sandbox tests also need `bubblewrap` and a host that permits its user namespaces.
Additional native libraries are determined by the build dependencies. Install Python
through your operating system or the official Python distribution. Verify the
development tools before building:

```sh
rustc --version
cargo --version
node --version
npm --version
python3 --version
```

Prebuilt release installations do not require these development tools; see
[installation](installation.md).

## Rust

```sh
cargo check --manifest-path kyler-rs/Cargo.toml --workspace
cargo build --manifest-path kyler-rs/Cargo.toml -p kyler-cli
cargo fmt --manifest-path kyler-rs/Cargo.toml --all -- --check
cargo test --manifest-path kyler-rs/Cargo.toml -p kyler-config
cargo test --manifest-path kyler-rs/Cargo.toml -p kyler-cli --test service
```

Run the affected crate tests for scoped changes. Changes spanning multiple crates
require the corresponding integration suites or the complete workspace suite:

```sh
cargo test --manifest-path kyler-rs/Cargo.toml --workspace
```

Integration tests start local HTTP servers, subprocesses, PTYs, and native sandboxes.
Restricted execution environments may need permission to run those checks. Use
`--offline` only when the needed dependencies are already cached.

## Web

Start the Web development server in a separate terminal after configuring and starting
the backend:

```sh
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

The frontend reads the backend connection from server-side configuration. The
development Web listener requires an available port. Installed and development services
must use separate ports when running concurrently. Integration tests must use isolated
workspaces and session stores.

## Configuration and local execution

For first-time backend setup from a checkout:

```sh
cargo run --manifest-path kyler-rs/Cargo.toml -p kyler-cli -- configure --addresses
cargo run --manifest-path kyler-rs/Cargo.toml -p kyler-cli -- serve
```

Both services use `~/.kyler/config.toml` by default. Skip `configure` when a valid
configuration already exists. Setup writes user configuration; use explicit `--config`
with a separate file to isolate development settings. See
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
complete-bundle fixture tests. Installer tests verify package activation, configuration
preservation, service startup, and rollback. Release validation also requires a
standalone Web startup check against the native bundle.

See [releasing](releasing.md) for producing a native package. Documentation validation
covers links, repository paths, and consistency with command-line help.
Documentation-only changes do not require an application build.

## Working files

Rust build output is stored in `kyler-rs/target/`, Next.js output in `web/.next/`, and
release output in `dist/`. Private GAIA datasets and local reports stay in the ignored
directories under `kyler-rs/crates/kyler-evals/`. Local data must be preserved during
directory moves and build cleanup.
