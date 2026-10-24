# Release + CI tooling for Ferry (state as of 2026-10-23)

Scope: how to ship `ferry` (crate `ferry-rs`, repo `patwrall/ferry`) with release-plz + dist, and which
pinned CI building blocks to use. Each claim has a primary source or a local measurement.
Scratch experiments are in `$SCRATCH/research-release-ci/`: a fake `ferry-rs` crate with
clap/serde/toml/rustix/thiserror/anyhow, run against dist 0.33.1, release-plz 0.4.0, cargo-deny 0.20.2,
actionlint 1.7.12 and zizmor 1.30.1. `deny.toml` was also re-checked against a scratch copy of the repo's
real `Cargo.toml` + `Cargo.lock` (`rust-version = "1.88"`, lib `ferry` + bin `ferry`).

## TL;DR

- **The split:** release-plz opens the release PR (version bump + `CHANGELOG.md`). When it merges,
  release-plz publishes to crates.io via trusted publishing and pushes the `vX.Y.Z` tag, but does not
  create a GitHub Release (`git_release_enable = false`). The tag push runs dist's `release.yml`, which
  builds 6 targets, creates the GitHub Release, and uploads the shell installer, `sha256.sum` and the
  Homebrew formula.
- **Token:** the tag must be pushed with a **GitHub App token** (or a PAT). A tag pushed with
  `GITHUB_TOKEN` never starts `release.yml`, and a PR opened with `GITHUB_TOKEN` gets CI runs that wait
  for manual approval.
- **Three things that will bite right now:**
  1. dist 0.33.1 builds `aarch64-apple-darwin` on `macos-14`, which GitHub retires on 2026-11-02
     (brownouts are already running). Override it to `macos-15`.
  2. `ubuntu-latest` moves to 26.04 between Oct 19 and Nov 19, 2026. Pin runner labels.
  3. Both rust-cache and rust-toolchain export `CARGO_INCREMENTAL=0`, which silently switches off what
     Ferry's incremental-reuse tests measure.
- **Pinning:** dist regenerates `release.yml`, and any hand edit makes `dist plan` fail. Pin its actions
  in `[dist.github-action-commits]` and keep Dependabot away from that file.

## 1. dist (axodotdev, formerly cargo-dist)

VERIFIED:
- **Version:** latest is **v0.33.1** (2026-10-21, dependency updates only;
  [release](https://github.com/axodotdev/cargo-dist/releases/tag/v0.33.1)). v0.33.0 (2026-09-10) added
  Azure signing and moved the shell installer's `env` script.
  - crates.io stops at **0.32.0** (crates.io API), so `cargo install cargo-dist` gets an old version.
    nixpkgs has 0.33.0.
  - Install from the release assets (`cargo-dist-<target>.tar.xz` + `.sha256`, or `cargo-dist-installer.sh`
    as CI does). The binary is `dist`.
- **Exact version match is enforced.** Measured: with `cargo-dist-version = "0.33.1"`, a 0.33.0 binary fails
  with `× You're running dist 0.33.0, but 'cargo-dist-version = 0.33.1' is set`.
- **Config:** `dist init` writes `dist-workspace.toml` (`[workspace] members = ["cargo:."]` + `[dist]`) and adds
  `[profile.dist] inherits = "release", lto = "thin"` to `Cargo.toml`.
  `[workspace.metadata.dist]` is still read during the config migration
  ([config.md](https://github.com/axodotdev/cargo-dist/blob/v0.33.1/book/src/reference/config.md)), and
  `dist migrate` converts between them.
- **Non-interactive setup:** `dist init --yes -c github -i shell -i homebrew -t ... --hosting github` works, but
  measured locally it **added `x86_64-pc-windows-msvc`** to the six `-t` targets. Better: write the config by
  hand (§10), then run `dist generate`.
- **Default runners** (`src/backend/ci/github.rs`, unchanged on `main`):
  - x86_64 linux (gnu + musl) and the plan/host jobs: `ubuntu-22.04`
  - aarch64 linux: `ubuntu-22.04-arm`, a **native ARM runner, not cross**. musl builds natively with apt `musl-tools`.
  - aarch64-apple-darwin: `macos-14`; x86_64-apple-darwin: `macos-15-intel`
  - Measured matrix with my override (`dist plan --output-format=json`): `macos-15`, `ubuntu-22.04-arm` ×2,
    `macos-15-intel`, `ubuntu-22.04` ×2.
- **macos-14 retirement:** deprecated 2026-07-06, retired **2026-11-02**, brownouts Oct 23/26/29/30
  ([runner-images#13518](https://github.com/actions/runner-images/issues/13518)). Override with
  `[dist.github-custom-runners] aarch64-apple-darwin = "macos-15"`.
  dist's known-runner table ("last updated 2025-10-26") includes macos-15/26 and ubuntu-24.04[-arm], but
  **not ubuntu-26.04 or macos-26-intel**.
- **Installers:** `installers = ["shell", "homebrew"]`, `tap = "patwrall/homebrew-tap"`, `publish-jobs = ["homebrew"]`.
  - The `publish-homebrew-formula` job checks out the tap with `secrets.HOMEBREW_TAP_TOKEN` and runs `git push`.
    The docs say to use a PAT with `repo` scope
    ([homebrew.md](https://github.com/axodotdev/cargo-dist/blob/v0.33.1/book/src/installers/homebrew.md)).
    Prereleases are skipped unless `publish-prereleases = true`.
  - Artifact names follow the **package** name (`ferry-rs-<target>.tar.xz`, `ferry-rs-installer.sh`).
    `formula = "ferry"` yields `ferry.rb` / `class Ferry`.
  - Measured: the installer checks glibc and falls back to the `*-musl-static` archive when glibc is too old.
    The formula uses the gnu archives on Linux.
- **Checksums:** `checksum = "sha256"` (the default) writes a per-archive `.sha256` plus a unified `sha256.sum`.
  Other options: sha512, sha3-*, blake2*, false.
  `github-attestations = true` is experimental; it adds `actions/attest` and gives build jobs
  `attestations: write` + `id-token: write`.
- **Pinning:** `[dist.github-action-commits]` (since 0.29) overrides the default tags (checkout@v6,
  upload-artifact@v7, download-artifact@v8, attest@v4, swatinem/rust-cache@v2, setup-node@v6, azure/login@v2).
  - **Measured, undocumented:** the value is inserted verbatim, so `"<sha> # v7.0.1"` produces
    `uses: actions/checkout@<sha> # v7.0.1`. `dist plan` passes and the YAML parses.
  - With the §10 config all 19 `uses:` lines came out pinned; actionlint reported only shellcheck style notes.
- **Never hand-edit the generated file.** Measured: adding one `# v7.0.1` comment by hand made `dist plan` exit 255
  ("has out of date contents and needs to be regenerated"). `allow-dirty = ["ci"]` disables the check, but the
  docs discourage it.
- **Triggers and release:**
  - Triggers: `pull_request` (runs `dist plan` only, per `pr-run-mode = "plan"`) and
    `push: tags: '**[0-9]+.[0-9]+.[0-9]+*'` (matches `v0.2.0`).
  - The `host` job creates the GitHub Release with `gh release create <tag> --target <sha> ... artifacts/*`.
    `gh` creates a draft, uploads, then publishes (`cli/cli pkg/cmd/release/create/create.go`).
  - Release notes come from the matching `CHANGELOG.md` section. Measured: dist parses release-plz's
    `## [0.1.0](url) - 2026-10-23` heading correctly.
- **Permissions it generates:** top level `contents: write`; build jobs `contents: read, attestations: write, id-token: write`.
  zizmor reports 3 errors on the generated file (excessive-permissions, template-injection, unpinned-images on
  the `container:` expression). These can't be fixed without `allow-dirty`.
- **Local dry run (measured):** `dist plan` ~1.0 s.
  `dist build --artifacts=local --target=x86_64-unknown-linux-gnu` 9.1 s → `target/distrib/*.tar.xz` + `.sha256`.
  `dist build --artifacts=global` 0.5 s → installer `.sh`, `ferry.rb`, `sha256.sum`, source tarball.
- **With release-plz:** dist's `gh release create` fails if a release already exists for the tag, so set
  release-plz `git_release_enable = false` (the common pattern). release-plz has an **open, unmerged** PR,
  [#3097](https://github.com/release-plz/release-plz/pull/3097), to drive dist natively. Don't depend on it.

## 2. release-plz

VERIFIED:
- **Versions:** CLI **0.4.0** (2026-10-22; breaking changes are library-only).
  `release-plz/action` **v0.5.145** (2026-10-23) defaults to release-plz `0.4.0` (`action.yml` input `version`),
  and the floating `v0.5` tag points at the same commit.
- **Config file:** `release-plz.toml` (or `.release-plz.toml`, `.config/release-plz.toml`;
  [config.md](https://release-plz.dev/docs/config)).
  - Fields and defaults: `git_release_enable` (true), `git_tag_enable` (true), `git_tag_name`
    (`v{{ version }}` for a single crate), `publish` (true), `pr_labels` ([]), `semver_check`, `changelog_update`.
  - `release_always` defaults to **true**. With `false`, it releases only when the release PR merges, detected
    by a `release-plz-` branch prefix.
  - The `[changelog]` section is rendered by git-cliff. The format is Keep a Changelog from Conventional
    Commits: `feat` → minor, `fix` → patch, `!` → major ([format.md](https://release-plz.dev/docs/changelog/format)).
  - Measured: unknown keys are rejected (`unknown field 'bogus_key'`), and `release-plz update` ran clean with §10.
- **Tokens** ([GitHub docs reusable](https://github.com/github/docs/blob/main/data/reusables/actions/actions-do-not-trigger-workflows.md)):
  - Events from `GITHUB_TOKEN` start no workflow runs, except `workflow_dispatch` and `repository_dispatch`.
  - New since ~2026-05 (docs PR #61001, flag `actions_requires_approval_for_actions_bot_prs`): a PR created or
    updated with `GITHUB_TOKEN` starts `pull_request` runs, but they are **approval-required**.
  - Fix: a GitHub App token or PAT ([token.md](https://release-plz.dev/docs/github/token)).
    App permissions: **Contents R/W + Pull requests R/W**, plus Administration R/W only if tags are protected.
  - `actions/create-github-app-token` v3: `client-id` replaces the deprecated `app-id` (v3.1.0).
    `permission-<name>: write` inputs narrow the token. It runs on node24.
- **crates.io trusted publishing** (crates.io source `svelte/src/routes/docs/trusted-publishing/+page.svelte`):
  - The crate **must already exist** ("initial publish requires an API token").
  - Settings → Trusted Publishing asks for owner, repo, **workflow filename**, and an optional environment.
  - Jobs need `id-token: write`; the token lasts 30 min.
  - The match is on the `workflow_ref` claim, i.e. the top-level workflow file (`crates_io_trustpub/src/github/claims.rs`).
  - A per-crate "Trusted Publishing only" switch turns off API tokens (migration `2025-11-19 add_trustpub_only_to_crates`).
  - `ferry-rs` is unclaimed (HTTP 404 on 2026-10-23); `ferry` is taken.
- **release-plz does the OIDC exchange itself.** Don't use `crates-io-auth-action` or set `CARGO_REGISTRY_TOKEN`;
  just give the job `id-token: write` ([quickstart](https://release-plz.dev/docs/github/quickstart)).
  Source (`release_plz_core/src/command/release.rs`): it uses trusted publishing only with no token, crates.io,
  and GitHub Actions. On failure it **warns and continues without a token**, so a misconfiguration surfaces at
  `cargo publish`.

## 3. Pinned action SHAs

Tags are the latest stable as of 2026-10-23. SHAs come from
`git ls-remote https://github.com/<repo> refs/tags/<tag> refs/tags/<tag>^{}`; annotated tags (marked "ann.")
show the peeled commit.

| action | tag | commit SHA | notes |
|---|---|---|---|
| actions/checkout | v7.0.1 | `3d3c42e5aac5ba805825da76410c181273ba90b1` | node24. v7 blocks fork-PR checkout in `pull_request_target`/`workflow_run` |
| Swatinem/rust-cache | v2.9.3 | `d1527c8f67ef9af357290475d4af4b0fab8f12e0` | ann. Always exports `CARGO_INCREMENTAL=0` (`src/restore.ts`) |
| dtolnay/rust-toolchain | v1 (=master) | `683b588c487a69d01f6c6ddf40f036ff682cc2c8` | `toolchain:` input is **required** here (only the `stable` branch defaults it). Sets `CARGO_INCREMENTAL=0` if unset |
| taiki-e/install-action | v2.87.31 | `1e29709c0ec63babbe8f266397a465f29a825f91` | has manifests for cargo-deny, cargo-hack, mdbook, typos, zizmor. Use `fallback: none` |
| EmbarkStudios/cargo-deny-action | v2.1.1 | `3c6349835b2b7b196a839186cb8b78e02f7b5f25` | ann. Docker image `rust:1.85.0-alpine` + cargo-deny 0.20.2 |
| crate-ci/typos | v1.51.1 | `7f9d0819ca1c008ade73f4b41fa9d258d48f605b` | ann. Version hard-coded in `action/entrypoint.sh`, so a SHA pin is safe |
| actions/configure-pages | v6.0.0 | `45bfe0192ca1faeb007ade9deae92b16b8254a0d` | node24 |
| actions/upload-pages-artifact | v5.0.0 | `fc324d3547104276b827a68afc52ff2a11cc49c9` | uses upload-artifact v7 |
| actions/deploy-pages | v5.0.1 | `368f82528645a54fb793d4d04e342629a3f51346` | node24 |
| release-plz/action | v0.5.145 | `7ef5e4d4b0420630cfcaeb182c01e13651d4c577` | ann. Default release-plz 0.4.0 |
| actions/create-github-app-token | v3.3.0 | `64ab729a6294000f5aa4a4bc0b73d1f32bc45510` | node24 |
| actions/upload-artifact | v7.0.2 | `cf430e030ddbb5b0abf93d22962f4752f3646cd9` | dist |
| actions/download-artifact | v8.0.2 | `9000827ccba6bdab643e8b6fd33ac0654aef8333` | dist |
| actions/attest | v4.2.2 | `1e69f48acb82d1966a394da916b4c1698aa569d6` | dist (attestations) |
| peaceiris/actions-mdbook | v2.0.0 | `ee69d230fe19748b7abf22df32acaa93833fad08` | ann. **Avoid:** node20, last release 2024-04 |
| rust-lang/crates-io-auth-action | v1.0.6 | `3268eb413133e2960fa4ae0e963ca76380c54cd9` | not needed with release-plz |

Node 20 was removed from runners on 2026-09-23
([changelog](https://github.blog/changelog/2026-09-23-node-20-is-no-longer-available-in-github-actions/)).

## 4. Least-privilege permissions

- **CI:** workflow-level `contents: read`.
- **Pages:** build job `contents: read`. Deploy job **`pages: write` + `id-token: write`** with
  `environment: github-pages` ([deploy-pages README](https://github.com/actions/deploy-pages/blob/v5.0.1/README.md)).
  One-time setting: Pages source = "GitHub Actions".
- **release-plz with an App token:** `GITHUB_TOKEN` needs only `contents: read` (checkout); writes go through the
  App token. `id-token: write` goes **only on the `release` job**.
  The upstream template that uses only `GITHUB_TOKEN` grants `contents: write` (+ `pull-requests: read`) on
  release and `contents: write, pull-requests: write` on release-pr.
- **dist `release.yml`:** whatever it generates (§1).

## 5. cargo-deny 0.20.2 (2026-07-09)

Measured on the scratch crate: `cargo deny check` → "advisories ok, bans ok, licenses ok, sources ok";
`-D warnings` exits 0.
- **`version = 2` is obsolete.** Since 0.16.0 the old fields are errors and v2 semantics are the only semantics
  ([CHANGELOG](https://github.com/EmbarkStudios/cargo-deny/blob/main/CHANGELOG.md)). The `cargo deny init`
  template has no `version` key, and measured, `version = 2`, `= 3`, and omitting it all behave the same.
  All advisory kinds are `deny` by default.
- **Licenses in this dependency set** (`cargo deny list`): MIT, Apache-2.0,
  `Apache-2.0 WITH LLVM-exception` (rustix/linux-raw-sys, both with an `OR MIT` alternative), and
  **Unicode-3.0** (unicode-ident). Dropping Unicode-3.0 gives `error[rejected] ... unicode-ident`, so the
  minimal allow list is `["MIT", "Apache-2.0", "Unicode-3.0"]`.
  Against the repo's real lockfile (adds serde_json, libc, tempfile, anstream) the same config passes; the only
  new license is `Unlicense OR MIT` (memchr), which MIT already satisfies.
- **0.20 CLI change:** `--config` and `--locked` moved to the root, i.e. `cargo deny --locked check`.
  Measured: `cargo deny check --config X` is now a parse error.
- **Prefer install-action** (`cargo-deny@0.20.2`, runs on the runner's toolchain) over cargo-deny-action, which
  builds a Docker image on rust 1.85 every run.

## 6. typos, Dependabot, Renovate

- **typos 1.51.1** reads `typos.toml`, `_typos.toml`, `.typos.toml` or `Cargo.toml [*.metadata.typos]`
  ([reference](https://github.com/crate-ci/typos/blob/v1.51.1/docs/reference.md)).
  Mark words valid with `[default.extend-words] word = "word"`; exclude files with `[files] extend-exclude`.
  Measured: 40-hex SHAs in workflows are not flagged. Upstream warns that floating `@v1` breaks CI when new typos land.
- **Dependabot** ([options](https://github.com/github/docs/blob/main/content/code-security/reference/supply-chain-security/dependabot-options-reference.md)):
  - Options: `groups` (`patterns`, `update-types`), `exclude-paths` (globs per ecosystem), and `cooldown`
    (3-day default where enabled).
  - It updates SHA pins **and** the same-line version comment (`@<sha> # v1.2.3`). A SHA no tag points to gets
    bumped to the latest commit
    ([caveats](https://github.com/github/docs/blob/main/data/reusables/actions/dependabot-version-updates-actions-caveats.md)).
  - **Exclude `.github/workflows/release.yml`** (any edit breaks `dist plan`).
- **Renovate** (alternative): `extends: ["helpers:pinGitHubActionDigestsToSemver"]`
  (preset exists in `lib/config/presets/internal/helpers.preset.ts`).

## 7. MSRV

- **Resolver v3 keeps the lockfile MSRV-compatible.** Edition 2024 → resolver "3" → MSRV-aware resolution.
  Measured: with `rust-version = "1.88"`, cargo 1.99 `generate-lockfile` printed
  "Locking 41 packages to highest Rust 1.88 compatible versions".
  `cargo +1.88 check --locked` then passed (6.5 s cold; toolchain install 8.7 s).
- **Floor:** the highest `rust-version` among current deps is **1.85** (clap 4.6, toml 1.1, hashbrown 0.17), the
  same as edition 2024's minimum. So MSRV ≥ 1.85.
- **`cargo hack check --rust-version --locked`** (0.6.45) reads `package.rust-version` and installs a missing
  toolchain via `rustup toolchain add <v> --no-self-update` (`src/rustup.rs`). Measured: it ran
  `rustup run 1.88 cargo check --locked`. No MSRV is duplicated in YAML.
- Alternative: `dtolnay/rust-toolchain@<sha>` with `toolchain: "1.88"` + `cargo check --locked`.

## 8. mdBook 0.5.4 (2026-07-06)

- **0.5.0 is a breaking major** ([migration guide](https://github.com/rust-lang/mdBook/blob/master/CHANGELOG.md#05-migration-guide)).
  - **Unknown config keys are errors.** Removed: `book.multilingual` (which 0.4's `mdbook init` wrote),
    `output.html.curly-quotes` (use `smart-punctuation`, now on by default), `copy-fonts`, google-analytics.
    Measured: `multilingual`/`curly-quotes` fail with `unknown field ...`.
  - Also: `hash-files` defaults to true; missing preprocessors error unless `optional = true`; relative `command`
    paths resolve from the book root; `--dest-dir` is relative to the cwd; Font Awesome 6 SVGs; `mdbook-` id prefix.
- **Measured:** 0.5.4 `mdbook init` writes `[book] title, authors, language`; a book with `site-url = "/ferry/"`
  builds fine.
- **For project Pages:** set `site-url = "/ferry/"` (needed for the 404 page;
  [CI guide](https://github.com/rust-lang/mdBook/blob/master/guide/src/continuous-integration.md)).
  Install via install-action `mdbook@0.5.4`.

## 9. Unprivileged user namespaces on GitHub Ubuntu runners

- **Ubuntu 24.04 restricts them** with `kernel.apparmor_restrict_unprivileged_userns=1`, and the runner image keeps
  it ([runner-images#10443](https://github.com/actions/runner-images/issues/10443); PR #11489 to change the image
  was closed unmerged). `unshare(CLONE_NEWUSER)` lands in a confined profile with no capabilities, so the mount
  namespace or mount fails with EPERM.
  - Per-job fix: **`sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`** (runners have passwordless sudo).
  - `kernel.unprivileged_userns_clone` already defaults to 1.
- **`ubuntu-latest` → 26.04 (kernel 7.0)** between 2026-10-19 and 2026-11-19
  ([#14748](https://github.com/actions/runner-images/issues/14748)). NOT VERIFIED from a primary source: a secondary
  write-up says 26.04 keeps the restriction. The guarded sysctl below handles both cases.
- Measured locally (NixOS 6.18): `unshare --user --map-root-user --mount true` works and the sysctl doesn't exist,
  hence the existence check.

## 10. Snippets

`dist-workspace.toml`. Measured: `dist generate` + `dist plan` exit 0, 19/19 actions pinned.
```toml
[workspace]
members = ["cargo:."]
[dist]
cargo-dist-version = "0.33.1"          # must equal local `dist --version`
ci = "github"
installers = ["shell", "homebrew"]
targets = ["aarch64-apple-darwin", "x86_64-apple-darwin", "aarch64-unknown-linux-gnu",
           "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-musl", "x86_64-unknown-linux-musl"]
tap = "patwrall/homebrew-tap"
formula = "ferry"
publish-jobs = ["homebrew"]
install-path = "CARGO_HOME"
install-updater = false
hosting = "github"
checksum = "sha256"
pr-run-mode = "plan"
github-attestations = true
[dist.github-custom-runners]
aarch64-apple-darwin = "macos-15"      # default macos-14 retires 2026-11-02
[dist.github-action-commits]           # text after the SHA is emitted verbatim -> YAML comment
"actions/checkout" = "3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1"
"actions/upload-artifact" = "cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2"
"actions/download-artifact" = "9000827ccba6bdab643e8b6fd33ac0654aef8333 # v8.0.2"
"actions/attest" = "1e69f48acb82d1966a394da916b4c1698aa569d6 # v4.2.2"
```

`release-plz.toml` (measured: accepted by 0.4.0):
```toml
[workspace]
git_release_enable = false   # dist creates the GitHub Release on the tag push
git_tag_enable = true
git_tag_name = "v{{ version }}"
release_always = false       # release only when the release PR is merged
semver_check = false         # lib `ferry` is internal; set true if it becomes a public API
pr_labels = ["release"]
[changelog]
protect_breaking_commits = true
```

`.github/workflows/release-plz.yml` (actionlint clean; zizmor clean apart from a YAML-anchor warning):
```yaml
name: Release-plz
on: { push: { branches: [main] } }
permissions: {}
jobs:
  release-plz-release:
    if: github.repository_owner == 'patwrall'
    runs-on: ubuntu-24.04
    permissions: { contents: read, id-token: write }   # id-token: crates.io trusted publishing
    steps:
      - &app-token
        id: app-token
        uses: actions/create-github-app-token@64ab729a6294000f5aa4a4bc0b73d1f32bc45510 # v3.3.0
        with:
          client-id: ${{ vars.RELEASE_APP_CLIENT_ID }}
          private-key: ${{ secrets.RELEASE_APP_PRIVATE_KEY }}
          permission-contents: write
          permission-pull-requests: write
      - &checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with: { fetch-depth: 0, persist-credentials: false }
      - &rust
        uses: dtolnay/rust-toolchain@683b588c487a69d01f6c6ddf40f036ff682cc2c8 # v1
        with: { toolchain: stable }
      - uses: release-plz/action@7ef5e4d4b0420630cfcaeb182c01e13651d4c577 # v0.5.145
        with: { command: release }
        env: { GITHUB_TOKEN: "${{ steps.app-token.outputs.token }}" }  # App token => tag triggers release.yml
  release-plz-pr:
    if: github.repository_owner == 'patwrall'
    runs-on: ubuntu-24.04
    permissions: { contents: read }
    concurrency: { group: "release-plz-${{ github.ref }}", cancel-in-progress: false }
    steps:
      - *app-token
      - *checkout
      - *rust
      - uses: release-plz/action@7ef5e4d4b0420630cfcaeb182c01e13651d4c577 # v0.5.145
        with: { command: release-pr }
        env: { GITHUB_TOKEN: "${{ steps.app-token.outputs.token }}" }
```

`.github/workflows/ci.yml`, core (actionlint + zizmor clean). The `lint` job (ubuntu-24.04) uses install-action
`tool: "cargo-deny@0.20.2,mdbook@0.5.4", fallback: none`, then runs `cargo deny --locked check`,
`crate-ci/typos@7f9d0819… # v1.51.1` and `mdbook build docs`.
```yaml
name: CI
on: { pull_request: {}, push: { branches: [main] } }
permissions: { contents: read }
jobs:
  test:
    strategy: { fail-fast: false, matrix: { os: [ubuntu-24.04, ubuntu-24.04-arm, macos-15] } }
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with: { persist-credentials: false }
      - uses: dtolnay/rust-toolchain@683b588c487a69d01f6c6ddf40f036ff682cc2c8 # v1
        with: { toolchain: stable, components: "clippy, rustfmt" }
      - uses: Swatinem/rust-cache@d1527c8f67ef9af357290475d4af4b0fab8f12e0 # v2.9.3
      - name: Allow unprivileged user namespaces (Ubuntu AppArmor)
        if: runner.os == 'Linux'
        run: |
          if [ -e /proc/sys/kernel/apparmor_restrict_unprivileged_userns ]; then
            sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
          fi
          unshare --user --map-root-user --mount true   # smoke test
      - run: cargo fmt --all --check
      - run: cargo clippy --all-targets --locked -- -D warnings
      - run: cargo test --all-targets --locked
```
The `msrv` job is checkout + rust-toolchain `stable` + install-action `cargo-hack@0.6.45`, then
`cargo hack check --rust-version --all-targets --locked`.

Pages deploy job. The build job runs `mdbook build docs`, then configure-pages `@45bfe019… # v6.0.0` and
upload-pages-artifact `@fc324d35… # v5.0.0` with `path: docs/book`.
```yaml
  deploy:
    needs: build
    runs-on: ubuntu-24.04
    permissions: { pages: write, id-token: write }
    environment: { name: github-pages, url: "${{ steps.deployment.outputs.page_url }}" }
    steps:
      - id: deployment
        uses: actions/deploy-pages@368f82528645a54fb793d4d04e342629a3f51346 # v5.0.1
```

`deny.toml` (measured: passes, including with `-D warnings`):
```toml
[graph]
targets = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-musl",
           "aarch64-unknown-linux-musl", "x86_64-apple-darwin", "aarch64-apple-darwin"]
all-features = true
[advisories]
unmaintained = "workspace"
yanked = "deny"
[licenses]
allow = ["MIT", "Apache-2.0", "Unicode-3.0"]
confidence-threshold = 0.93
[bans]
multiple-versions = "warn"
wildcards = "deny"
allow-wildcard-paths = true
[sources]
unknown-registry = "deny"
unknown-git = "deny"
```

`.github/dependabot.yml`:
```yaml
version: 2
updates:
  - package-ecosystem: cargo
    directory: /
    schedule: { interval: weekly }
    groups: { cargo-minor-patch: { update-types: [minor, patch] } }
  - package-ecosystem: github-actions
    directory: /
    schedule: { interval: weekly }
    exclude-paths: [".github/workflows/release.yml"]   # dist-generated; bump pins in dist-workspace.toml
    groups: { actions: { patterns: ["*"] } }
```

Other files:
- `docs/book.toml` (0.5 keys only): `[book] title = "Ferry"`, `[output.html] site-url = "/ferry/"`,
  `git-repository-url = "https://github.com/patwrall/ferry"`.
- `_typos.toml`: `[files] extend-exclude = ["CHANGELOG.md"]`, plus `[default.extend-words]` for jargon.

## Could not verify

- That Ubuntu 26.04 runners keep `apparmor_restrict_unprivileged_userns=1`. Only a secondary source says so;
  nothing was run on a real GitHub runner.
- That a **fine-grained** PAT (Contents R/W on `patwrall/homebrew-tap` only) works as `HOMEBREW_TAP_TOKEN`.
  dist's docs say classic `repo`. It should work (checkout + push only), but it's untested.
- Whether step-level `env: CARGO_INCREMENTAL` overrides the value rust-cache exports via `GITHUB_ENV`.
- How Dependabot treats `dtolnay/rust-toolchain@<sha> # v1` (a moving major tag with no semver releases).
- dist + release-plz end to end on real GitHub, including with "immutable releases" on.
  gh's draft-then-publish flow suggests it's compatible.
- When `ubuntu-22.04` (dist's default gnu runner) will be deprecated. With 26.04 GA, GitHub's two-GA-images
  policy implies a notice should follow; none found yet.

## Design implications for Ferry

1. **Adopt release-plz + dist** as in the TL;DR. Set `git_release_enable = false` and `release_always = false`;
   dist owns the GitHub Release, installers and Homebrew. No custom release pipeline.
2. **Create a GitHub App** (Contents R/W, Pull requests R/W, installed only on `patwrall/ferry`). Store
   `RELEASE_APP_CLIENT_ID` as a variable and `RELEASE_APP_PRIVATE_KEY` as a secret; use it in both release-plz jobs.
   Without it, dist never runs on tags and release-PR CI needs manual approval.
3. **First release by hand:**
   1. `cargo publish` `ferry-rs` 0.1.0 locally with a `publish-new` token. The name is free now; claim it soon.
   2. Push `v0.1.0` manually; `release.yml` runs.
   3. Add the crates.io trusted publisher: `patwrall` / `ferry` / `release-plz.yml`.
   4. Enable "Trusted Publishing only" and revoke the token.
4. **dist hygiene:**
   - Commit the generated `release.yml` and never hand-edit it.
   - Pin through `[dist.github-action-commits]` with `# vX` comments.
   - Override `aarch64-apple-darwin` to `macos-15` **now**.
   - Pin `cargo-dist-version` and install that exact binary from GitHub releases for local `dist plan`.
5. **Explicit runner labels** (`ubuntu-24.04`, `ubuntu-24.04-arm`, `macos-15`) in hand-written workflows. Add an
   `ubuntu-26.04` test leg before `ubuntu-latest` flips, since the namespace/overlay tests are kernel-sensitive.
6. **Namespace tests:** run the guarded sysctl, then fail fast with a clear message if `unshare -Urm` still fails.
   Keep a macOS leg that tests the non-namespace fallback (stamped mtimes + clonefile).
7. **The test harness must clean the env of the `cargo` it spawns:** remove `CARGO_INCREMENTAL` (forced to 0 by
   rust-cache and rust-toolchain), plus `CARGO_TARGET_DIR`, `RUSTFLAGS` and `CARGO_BUILD_*` that it doesn't own.
   Otherwise the canonical-path/incremental tests pass vacuously in CI.
   Ferry's runtime should probably warn when it inherits `CARGO_INCREMENTAL=0`, since agents run in CI-like envs.
8. **Use the §10 configs as-is:** `deny.toml` (MIT / Apache-2.0 / Unicode-3.0, Linux+macOS graph, no `version`
   keys), `_typos.toml`, and Dependabot with `release.yml` excluded and grouped updates.
9. **MSRV:** the repo already sets `rust-version = "1.88"`, which is above the dependency floor of 1.85 and was
   verified locally. Gate it with `cargo hack check --rust-version --locked`; resolver v3 keeps the lockfile compatible.
10. **Docs:** mdBook 0.5.4 via install-action, `site-url = "/ferry/"`, Pages deploy with `pages: write, id-token: write`.
    Not peaceiris/actions-mdbook.
11. **Install paths for the README:** `brew install patwrall/tap/ferry`, `cargo install ferry-rs`, and
    `curl … /releases/latest/download/ferry-rs-installer.sh | sh`. Tarballs are `ferry-rs-<target>.tar.xz`
    because the package is `ferry-rs`.

## Open questions

- Is Windows out of scope (no namespaces, no overlayfs)? If so, keep out the Windows target that `dist init --yes` adds.
- Enable the experimental `github-attestations` now, or wait?
- Run zizmor in CI? It would need to suppress the dist-generated `release.yml` findings.
- If release-plz#3097 lands, release-plz could drive dist directly (no `release.yml`, no App token for tags).
  Re-evaluate then.
- Why does dist stop publishing to crates.io after 0.32.0? Is it deliberate upstream?
