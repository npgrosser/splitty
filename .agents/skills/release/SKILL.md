---
name: release
description: Release Splitty by choosing or accepting a new version, committing and pushing its annotated Git tag to start GitHub Actions, building the desktop package for the current host, and copying it into Downloads. WSL targets the Windows host. Use for a requested Splitty release, not merely for editing this skill or discussing releases.
---

# Splitty release

Deliver a pushed release tag, its GitHub Actions run URL, and a locally built desktop installer/package in the current host's Downloads folder. Work in the Splitty repository selected by the user/current directory; do not hard-code a checkout, account, username or version.

An actual request to run this release workflow authorizes its version commit, annotated tag and GitHub push. Do not ask for the same approval again. Creating or editing this skill does not request a release. Respect narrower requests such as local-build-only or no-push. The repository's normal no-push convention for implementation/archive work does not override the user's explicit release-and-push request. The workflow publishes the GitHub release itself once every platform has built; installing or running the package requires a separate user request.

## Prepare

1. Read `CLAUDE.md`/`AGENTS.md` where present, `scripts/release.sh`, `scripts/windows-installer.sh`, and `.github/workflows/release.yml`. The current repository scripts and workflow are authoritative.
2. Inspect Git status, branch/upstream, GitHub remote, latest tags and unpushed commits. Review and commit the changes included in the release request with conventional commit messages. Preserve unrelated work; use an isolated clean release checkout if necessary. Do not silently release an older HEAD because intended changes are uncommitted, and do not stage everything, stash, reset or clean unrelated files to satisfy the release script.
3. Use the requested version. Otherwise select the next unused patch version above the current workspace version and stable release tags, and announce it. A new release needs a new version: the remote/WSL servers are installed by version. Verify the tag is unused locally and on the chosen GitHub remote. Resolve ambiguity about release content or target branch before tagging, but do all independent preparation first.
4. Run the appropriate checks from the repository instructions; reuse successful checks for unchanged content. Check Git/`gh` authentication and platform prerequisites. Resolve the helper paths relative to this `SKILL.md`, not the shell's working directory. Run `python3 <skill-dir>/scripts/downloads.py` (or `py -3` on native Windows) to discover the desktop OS and actual Downloads path. A failed Windows folder lookup is an error, never a reason to copy into WSL's Linux home.

## Create and push the release

From the clean release checkout, run `bash scripts/release.sh VERSION`. It updates `Cargo.toml`, `Cargo.lock`, Tauri and Electron versions, commits them and creates annotated tag `vVERSION`. Verify with `bash scripts/release.sh --check vVERSION` and record `git rev-parse 'vVERSION^{commit}'` as `release_sha`.

Push the release commit to the selected branch and only that tag in one atomic operation. For remote `origin` and the selected branch in `release_branch`:

```sh
git push --atomic origin "HEAD:refs/heads/$release_branch" "refs/tags/$tag:refs/tags/$tag"
```

Use the actual verified remote/branch, not necessarily `origin`/`main`. Do not use `--force`, `--tags`, move an existing tag or change unrelated refs. If a push fails or times out, inspect local and remote refs before retrying. Resume the same release when the remote already has the expected commit/tag; never create a second version just to retry. Stop on a conflicting tag or divergent branch and report the concrete conflict.

Verify the remote's peeled tag points to `release_sha`. Locate the `Release` run for this exact tag/commit, for example with `gh run list --workflow release.yml --commit "$release_sha" --event push --json databaseId,headSha,headBranch,status,conclusion,url`. Poll for registration for up to two minutes; report a missing run rather than claiming CI started. The tag push is the trigger; do not additionally dispatch the workflow. Keep the run URL and ID.

## Build for this host and deliver

Read [references/platform-builds.md](references/platform-builds.md) for the detected platform. Build the desktop package locally from **the exact release commit**. WSL means Windows NSIS, macOS means a native or universal DMG, and native Linux means Tauri deb/AppImage. Build Electron only when requested or when it is the established local variant. Downloading CI's completed desktop installer is not a substitute for the requested local build.

Use a dedicated clean checkout at the release commit if the working tree may change during the build. Keep dependency/build caches when safe, but select only artifacts from this successful build with the expected version/architecture; do not pick the newest file in a directory containing older builds. Do not use test-skipping flags unless requested. Record the commit, build command, output path and checksum.

Copy the explicitly selected package(s) with:

```sh
python3 <skill-dir>/scripts/downloads.py --copy /absolute/path/to/package
```

The helper resolves relocated Windows Downloads via the Windows shell, uses XDG Downloads on Linux, preserves the original build output and writes a SHA-256 sidecar. It refuses to overwrite an existing file with different content. For a deliberate rebuild of the same version, use a distinct version-and-commit filename instead of overwriting another download.

Monitor the matching CI run while building. The local delivery and CI have separate outcomes: finish the local package even if an unrelated CI platform fails, and report that failure. A successful local build alone is not a successful GitHub release. On interruption, resume unfinished build/delivery/checks against the same tag. Never delete or retag a published version to conceal a failure.

Finish with the tag and commit, push result, CI URL and actual status, local platform/architecture, and the full Downloads path(s) with SHA-256. Under WSL include the Windows-native path. State any uncompleted part and its concrete blocker, and whether the GitHub release was published.
