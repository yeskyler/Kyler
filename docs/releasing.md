# Build and publish releases

Building and publishing are separate operations. Packaging creates native archives;
publishing uploads existing archives to GitHub. Neither step installs or restarts
local services. Run these commands from the repository root.

## Scripts and prerequisites

| File | Purpose |
| --- | --- |
| `scripts/package.sh` / `package.py` | Build Rust and Web; bundle a native Node.js runtime |
| `scripts/release.sh` / `release.py` | Validate existing bundles, push a version tag, and publish a GitHub Release |
| `scripts/install.sh` | Download or install a bundle, configure it, and manage installed services |
| `scripts/tests/test_install.py` | Isolated installer regression tests |
| `scripts/tests/test_release.py` | Isolated publisher tests; no real uploads or tag pushes |

Build hosts need stable Rust/Cargo, native compilation tools, Python 3.9+, and
Node.js/npm. `--download-node` fetches the official latest Node.js 24 distribution
and verifies its checksum; it does not install Cargo, Python, or system compilers.
Publishing needs Git, Python 3.9+, and an authenticated [GitHub CLI](https://cli.github.com/).
End users installing a complete bundle need none of these development runtimes.

## 1. Prepare the source

Kyler uses the MIT License. Retain the committed `LICENSE` in public releases. Set the intended
version in `[workspace.package]` in `kyler-rs/Cargo.toml`, update the changelog, and
commit source changes. Keep the frontend package metadata consistent when bumping
its version. Run relevant checks from [development](development.md).

Build from a clean checkout of the release commit. Normal packaging records that
commit in `bundle.json`; dirty, exported, or reused builds have no verified source
revision and cannot be published by the release script.

## 2. Build native bundles

```sh
sh scripts/package.sh --download-node
```

Alternatively, supply an extracted official native Node.js distribution with
`bin/node`, `LICENSE`, and npm available:

```sh
sh scripts/package.sh --node-dist /path/to/extracted-node-distribution
```

Packaging builds `kyler-cli` in release mode, runs `npm ci` and `npm run build` in
`web/`, and collects the backend, standalone Web, native Node runtime, documentation,
and third-party notices. It does not contact GitHub or create a version tag.

```text
dist/
├── kyler-v0.1.0-aarch64-apple-darwin.tar.gz
├── SHA256SUMS
└── install.sh
```

The outer checksum file verifies downloadable archives. The archive also contains
an internal checksum file for extracted files. Environment files are excluded
from traced Web output, but maintainers must still inspect deliverables for private
data and smoke-test installation with a temporary HOME and test ports.

`--output PATH` changes the destination. `--skip-build` and `--binary PATH` can
package existing outputs for local testing; those bundles deliberately lack a
verified build revision. Use a normal build before publishing.

Build on each intended native platform. This script does not cross-compile or
produce a platform matrix automatically. To publish several platforms together,
use identical clean source commits and combine their `.tar.gz` archives and checksum
entries in one directory. Keep one matching `install.sh`. A Release contains only
the platforms you actually built.

## 3. Validate without publishing

```sh
sh scripts/release.sh 0.1.0 --dry-run
```

Validation checks the source version, clean working tree, origin repository,
archive checksums, platform/version metadata, source revision, project license,
and matching installer. `--artifacts PATH` selects another directory. Dry-run is
local only: no GitHub authentication, tag creation, push, or Release upload occurs.

## 4. Publish automatically with GitHub Actions

The repository's [Release workflow](../.github/workflows/release.yml) triggers only
for version tags (`v*` with a numeric prefix) and manual requests for an existing
tag. It does not run on ordinary branch pushes or pull requests.

After updating version metadata and the changelog, commit and push the source:

```sh
git tag -a v0.1.0 -m "Kyler 0.1.0"
git push origin v0.1.0
```

The tag must match `[workspace.package].version`; use a fresh matching version
for subsequent releases. Watch the repository's Actions tab for progress.

The workflow validates the tag and scripts, then runs four native builds:

| Runner | Archive target |
| --- | --- |
| `macos-15` | `aarch64-apple-darwin` |
| `macos-15-intel` | `x86_64-apple-darwin` |
| `ubuntu-24.04` | `x86_64-unknown-linux-gnu` |
| `ubuntu-24.04-arm` | `aarch64-unknown-linux-gnu` |

Each runner builds Rust and Web and bundles a matching native Node.js runtime.
Only when every build succeeds does a separate publishing job download the assets,
validate each checksum and build revision, merge the checksum manifests, require all
four targets, and call the independent publisher. There is no last-writer-wins
checksum upload: all platforms are published together in a single Release.

Build and validation jobs have read-only repository access. Only the publishing job
has `contents: write` and receives the built-in `GITHUB_TOKEN` as `GH_TOKEN`.
No personal access token or model key is needed. Actions are pinned to verified
commit hashes. The publisher requires the existing remote tag and will not recreate
it if it was deleted during the build.

To retry infrastructure failures, use Actions' rerun controls or manually run the
workflow with the same existing tag. Already-created Releases, including drafts,
are rejected instead of overwritten. Inspect a partial draft before recovery.
Artifact uploads are retained for seven days; published Release assets remain separate.

## 5. Publish local bundles instead

This is an alternative to the tag-triggered workflow, not an extra required step.
Do not run both publishers for the same version concurrently. The local command
creates and pushes a tag when needed; its Release upload happens in the same local
process. Tags pushed with your account can also trigger the workflow, so for local
publishing disable the Release workflow in GitHub Actions first. Prefer the tag
workflow for normal multi-platform releases.

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

The implementation uses [gh release create](https://cli.github.com/manual/gh_release_create)
and [gh release edit](https://cli.github.com/manual/gh_release_edit). It never
force-pushes tags, overwrites existing assets, or deletes an existing Release.
Running the publishing command is the explicit publication action; there is no
extra interactive confirmation.

If an upload fails, the tag or draft may remain. Inspect it with
`gh release view v0.1.0`; no automatic tag deletion or replacement is attempted.
Recover the draft deliberately rather than rerunning a command that would overwrite
published files. Existing Releases are rejected, including drafts.

`dist/` stays ignored by Git. Upload archives as Release assets, never as tracked
binaries. Users can then follow [installation](installation.md).
