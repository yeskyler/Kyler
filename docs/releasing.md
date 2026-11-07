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

Build hosts require stable Rust and Cargo, native compilation tools, Python 3.9+, and
Node.js with npm. `--download-node` fetches the official latest Node.js 24 distribution
and verifies its checksum; it does not install Cargo, Python, or system compilers.
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
sh scripts/package.sh --download-node
```

The `--node-dist PATH` option uses an extracted official native Node.js distribution.
The selected directory must contain `bin/node` and `LICENSE`; npm must be available on
the build host.

Packaging builds `kyler-cli` in release mode, runs `npm ci` and `npm run build` in
`web/`, and collects the backend, standalone Web, native Node runtime, documentation,
and third-party notices. It does not contact GitHub or create a version tag.

```text
dist/
├── kyler-v0.1.0-aarch64-apple-darwin.tar.gz
├── SHA256SUMS
└── install.sh
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
directory. Keep one matching `install.sh`. Each Release contains only the platform
archives supplied to the publisher.

## 3. Validate without publishing

```sh
sh scripts/release.sh 0.1.0 --dry-run
```

Validation checks the source version, clean working tree, origin repository, archive
checksums, platform/version metadata, source revision, project license, and matching
installer. `--artifacts PATH` selects another directory. Dry-run is local only: no
GitHub authentication, tag creation, push, or Release upload occurs.

## 4. Publish local bundles

The local publisher creates and pushes a version tag when needed, then uploads the
prepared bundles to GitHub in the same process. Build and validate all required platform
bundles before publishing. The version must match the workspace manifest and must not
already exist as a Release, including a draft.

```sh
gh auth login
sh scripts/release.sh 0.1.0
```

The script derives the GitHub repository from `origin` and performs these steps:

1. Authenticate and reject an existing Release with the same tag.
2. Verify existing local and remote tags point to the release commit.
3. Create an annotated `v0.1.0` tag if needed and push that tag. Branches are not pushed.
4. Generate a version-specific `SHA256SUMS`, leaving other versions out of that upload.
5. Upload native archives, checksums, and `install.sh` to a draft Release with generated notes.
6. Publish only after all uploads succeed. Versions with a prerelease suffix are marked prerelease.

Use `--draft` to retain the uploaded Release as a draft for inspection:

```sh
sh scripts/release.sh 0.1.0 --draft
```

The implementation uses [gh release
create](https://cli.github.com/manual/gh_release_create) and [gh release
edit](https://cli.github.com/manual/gh_release_edit). It never force-pushes tags,
overwrites existing assets, or deletes an existing Release. The publishing command
performs the publication operation without an additional interactive confirmation.

If an upload fails, the tag or draft may remain. Inspect it with `gh release view
v0.1.0`; no automatic tag deletion or replacement is attempted. Inspect and resolve any
partial draft before retrying publication. Existing Releases are rejected, including
drafts.

`dist/` stays ignored by Git. Upload archives as Release assets, never as tracked
binaries. Users can then follow [installation](installation.md).
