---
name: releasing-a-version
description: How to cut a new release of director-narrative-core — bump the version, tag it, and let the release pipeline build and publish the Windows publisher.exe to GitHub Releases. Use when asked to release a version, publish a build, or change the release workflow.
---

# Releasing a version

Releases are produced by [`.github/workflows/release.yml`](../../../.github/workflows/release.yml).
On a pushed `v*` tag (or manual `workflow_dispatch`) it builds the publisher on a native Windows
runner and publishes it to a
[GitHub Release](https://github.com/margic/director-narrative-core/releases).

## What ships

The one distributable binary is `src/bin/publisher.rs` → `publisher.exe` (the live iRacing
telemetry publisher that streams narrative events to the Race Control API). It only runs
meaningfully on Windows (iRacing shared memory). Each release attaches:

- `publisher-<tag>-x86_64-pc-windows-msvc.exe` — raw executable
- `publisher-<tag>-x86_64-pc-windows-msvc.zip` — exe + `LICENSE` + `README.md`
- `publisher-<tag>-x86_64-pc-windows-msvc.sha256` — SHA256 checksums

## How to cut a release

1. Bump `version` under `[package]` in the root `Cargo.toml`, commit, and merge to `main` via PR.
2. Tag the release commit with a matching `v`-prefixed semver tag and push:
   ```sh
   git tag v0.1.2
   git push origin v0.1.2
   ```
3. The workflow builds the exe and creates the Release. Pre-release tags (containing `-`, e.g.
   `v0.1.2-rc.1`) publish as GitHub pre-releases.

**Version guard:** the tag minus its leading `v` MUST equal the `Cargo.toml` version, or the run
fails fast. So always bump `Cargo.toml` first, then tag the same number.

To re-build an existing tag, use the workflow's manual `workflow_dispatch` (takes a `tag` input).

## Key facts about the pipeline

- Runs on `windows-latest` with the **MSVC** target `x86_64-pc-windows-msvc` (the canonical
  end-user Windows target). MSVC + the target are provided via `dtolnay/rust-toolchain`.
- Builds only the publisher: `cargo build --release --bin publisher --target
  x86_64-pc-windows-msvc`. The `publisher_ui` binary is feature-gated (`publisher-ui`) and is
  NOT part of the release.
- Publishes with `softprops/action-gh-release@v2`, `generate_release_notes: true`,
  `permissions: contents: write`. Uses the built-in `GITHUB_TOKEN` — **no extra secrets needed**
  for the current unsigned flow.

## Gotchas / conventions

- The exe is currently **unsigned** → Windows SmartScreen warns on first run. If Authenticode
  signing is ever added, sign the exe *after* `cargo build` but *before* staging/zipping so the
  raw exe AND the zipped copy are both signed and match the SHA256 checksums.
- The publisher reads credentials from `publisher.toml` next to the exe or from
  `PUBLISHER_AUTH_*` / `PUBLISHER_RC_API_URL` environment variables (see README § "Run against a
  live iRacing session") — no credentials are baked into the release.
- Validate workflow edits with `actionlint`. Real end-to-end verification = pushing a `v*` tag;
  a local proxy is `cargo build --release --bin publisher` on any Windows machine.
