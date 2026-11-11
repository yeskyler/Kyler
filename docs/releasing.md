# Release management

Building and publishing are separate operations. Packaging creates native archives;
publishing uploads existing archives to GitHub. Neither step installs or restarts local
services. Run these commands from the repository root.

## Scripts and prerequisites

| File | Purpose |
| --- | --- |
| `scripts/package.sh` / `package.py` | Build Rust and Web; bundle a native Node.js runtime |
| `scripts/release.sh` / `release.py` | Validate existing bundles, push a version tag, and publish a GitHub Release |
| `scripts/install.sh` | Download or install a bundle, configure it, and manage installed services |
| `scripts/tests/test_install.py` | Isolated installer regression tests |
| `scripts/tests/test_release.py` | Isolated publisher tests; no real uploads or tag pushes |
| `scripts/tests/test_package.py` | Isolated toolchain setup and runtime download tests |

Build hosts require native compilation tools and Python 3.9+. Packaging checks for
Rust and Cargo, including tools installed under `CARGO_HOME` or `~/.cargo/bin`. If
they are missing, it uses curl to download the official rustup installer and installs
the stable minimal toolchain without interactive prompts or shell profile changes.
The build process immediately uses the installed tools. Packaging prints nine numbered
steps and retains Cargo, npm, and download output. It first checks local Node.js and
reuses a native version 22 or newer, along with local npm. If the matching Node.js
license is not installed, only that version's license is downloaded for the bundle.
When local Node.js is missing or incompatible, packaging downloads the official latest
Node.js 24 distribution and verifies its checksum. Use `--download-node` to explicitly
request that official distribution instead. Missing npm alongside a compatible local
Node.js produces an actionable error; install npm or supply `--node-dist`. A system Node.js installation is not required. Python, curl,
and system compilers must already be available.
Publishing requires Git, Python 3.9+, and an authenticated [GitHub
CLI](https://cli.github.com/). Complete bundles include the runtime components required
for installation.

## 1. Prepare the source

Kyler uses the MIT License. Retain the committed `LICENSE` in public releases. Set the
release version in `[workspace.package]` in `kyler-rs/Cargo.toml`, update the changelog,
and commit source changes. Keep the frontend package metadata consistent when bumping
its version. Complete the validation requirements in [development](development.md).

Build from a clean checkout of the release commit. Normal packaging records that commit
in `bundle.json`; dirty, exported, or reused builds have no verified source revision and
cannot be published by the release script.

## 2. Build native bundles

```sh
sh scripts/package.sh
```

The `--node-dist PATH` option uses an extracted official native Node.js distribution.
The selected directory must contain `bin/node` and `LICENSE`, with npm available
for builds. The default official download includes npm. No download flag is needed.

Packaging builds `kyler-cli` in release mode, runs `npm ci` and `npm run build` in
`web/`, and collects the backend, standalone Web, native Node runtime, documentation,
and third-party notices. It does not contact GitHub or create a version tag.

```text
dist/
├── kyler-v0.1.0-aarch64-apple-darwin.tar.gz
└── SHA256SUMS
```

The outer checksum file verifies downloadable archives. The archive also contains an
internal checksum file for extracted files. Environment files are excluded from traced
Web output, and release validation must inspect deliverables for private data and verify
installation with a temporary HOME and dedicated test ports.

`--output PATH` changes the destination. `--skip-build` and `--binary PATH` can package
existing outputs for local testing; those bundles lack a verified source revision and
cannot be published by the release script.

Build on each intended native platform. This script does not cross-compile or produce a
platform matrix automatically. To publish several platforms together, use identical
clean source commits and combine their `.tar.gz` archives and checksum entries in one
directory. Publish `scripts/install.sh` separately on your installation server; neither
packaging nor release publishing distributes it. Each Release contains only the platform
archives supplied to the publisher.

## 3. Validate without publishing

```sh
sh scripts/release.sh 0.1.0 --dry-run
```

Validation checks the source version, clean working tree, origin repository, archive
checksums, platform/version metadata, source revision, and project license. `--artifacts PATH` selects another directory. Dry-run is local only: no
GitHub authentication, tag creation, push, or Release upload occurs.

## 4. Publish local bundles

The local publisher creates and pushes a version tag when needed, then uploads the
prepared bundles to GitHub in the same process. Build and validate all required platform
bundles before publishing. The version must match the workspace manifest. When the
Release already exists, packaging from the same clean source commit can add missing
platform archives to that Release.

```sh
gh auth login
sh scripts/release.sh 0.1.0
```

The script derives the GitHub repository from `origin` and performs these steps:

1. Authenticate and locate any existing Release with the same tag.
2. Verify existing local and remote tags point to the release commit.
3. Create an annotated `v0.1.0` tag if needed and push that tag. Branches are not pushed.
4. Generate a version-specific `SHA256SUMS`, leaving other versions out of that upload.
5. Upload native archives and checksums to a draft Release with generated notes.
6. Publish new drafts only after all uploads succeed. Versions with a prerelease suffix are marked prerelease.

For an existing Release, the remote tag must already point to the local source commit.
Missing platform archives are uploaded without replacing existing archives. Identical
archives are skipped; differing content is rejected before uploading.
The script downloads the current `SHA256SUMS`, preserves other platform entries, and
replaces only that shared manifest after confirming the new uploads. Older GitHub
asset metadata without a SHA256 digest is verified by downloading the asset.

Use the same publish command on each native build host, sequentially. The script
checks for concurrent asset changes before updating the manifest, but GitHub provides
no atomic transaction across these uploads. A published Release stays published; an
existing draft is published after successful completion unless `--draft` is selected.
Release notes are preserved when appending assets. GitHub API pagination does not
require the `gh api --slurp` option.

Use `--draft` to retain the uploaded Release as a draft for inspection:

```sh
sh scripts/release.sh 0.1.0 --draft
```

The implementation uses [gh release
create](https://cli.github.com/manual/gh_release_create) and [gh release
edit](https://cli.github.com/manual/gh_release_edit). It never force-pushes tags,
overwrites existing archives, or deletes an existing Release. The shared
checksum manifest is updated when platforms are added. The publishing command
performs the publication operation without an additional interactive confirmation.

If an upload fails, the tag or draft may remain. Inspect it with `gh release view
v0.1.0`; no automatic tag deletion or replacement is attempted. Retry from the same
clean source and artifacts. Uploaded identical files are skipped, and checksum entries
for completed remote archives can be recovered if a manifest upload failed. Incomplete
remote assets require inspection before retrying.

`dist/` stays ignored by Git. Upload archives as Release assets, never as tracked
binaries. Users can then follow [installation](installation.md).


## Installation server

Deploy the same archives and `SHA256SUMS` under
`/usr/share/caddy/releases/vVERSION/`, served at
`https://www.kyler.work/releases/vVERSION/`. Upload platform archives first, then the
checksum file. Finally, atomically publish `/usr/share/caddy/releases/latest.txt`
containing the version on one line, for example `v0.1.0`. Update this pointer only
after all intended platforms are available. Keep earlier version directories for
pinned installations.

Deploy `scripts/install.sh` separately as `/usr/share/caddy/install.sh`. Caddy must
allow `/install.sh` and `/releases/*`; directory listing is not required. The GitHub
publisher does not deploy files to this server. Installation prefers the server and
falls back to GitHub for the same selected version.
