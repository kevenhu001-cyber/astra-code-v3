# GitHub workflows

The CI / CD system is split into focused workflows. Each one owns a single
concern so it's obvious where a check belongs when something breaks.

## Index

| Workflow | Trigger | What it does | Expected cost |
|----------|---------|--------------|---------------|
| `format.yml` | PR, push to main | `cargo fmt --all -- --check` | ~30s |
| `lint.yml` | PR, push to main | `cargo clippy --workspace --all-targets -- -D warnings` | ~3 min |
| `test.yml` | PR, push to main | unit tests + regressions + auth + selected PTY E2E + leader PTY E2E | ~15–20 min |
| `build.yml` | PR, push to main, tag `v*` | Cross-platform release builds + GitHub release | ~25 min (5-platform matrix) |
| `audit.yml` | PR, push to main, weekly Mon 06:00 UTC | `cargo audit` + third-party-license consistency | ~2 min |
| `nightly.yml` | daily 04:00 UTC | full test + full scripted + full leader + benches + upstream probe | ~60–90 min |
| `docs.yml` | PR + main (paths-filtered) | user-guide link rot + shellcheck + pwsh parse + npm smoke + standalone marketing/brand tests | ~5 min; copy tests take seconds |

`dependabot.yml` (sibling, not a workflow) opens weekly PRs for cargo, npm,
and GitHub Actions version bumps.

## Layering

```
PR feedback  :  format → lint → test   (parallel)
                                  ↓
                              build    (5 platforms fan-out)
                                  ↓
                              release  (only on main / v* tag)

Weekly      :  audit                 (Mondays 06:00 UTC)

Daily       :  nightly               (04:00 UTC, the heavy tier)

On-demand   :  workflow_dispatch on every workflow above
```

## Conventions

- **Toolchain** is pinned to match the repo's `rust-toolchain.toml` (1.94.0).
  Keep `dtolnay/rust-toolchain@1.94.0` in sync when bumping that file.
  Version-specific action refs do not accept a `toolchain` input.
- **Concurrency**: every workflow uses `cancel-in-progress: true` so a stale
  push to a PR doesn't keep racking up minutes after a fix.
- **Permissions**: default `contents: read`. Only `build.yml`'s `release`
  job uses `contents: write` to publish the GitHub release.
- **Linux-only** by default; macOS/Windows only appear in the `build` matrix.
- **Cache**: Cargo jobs use `Swatinem/rust-cache@v2` with
  `cache-on-failure: true`. The docs marketing/brand tests use only `std`
  and run via `rustc --test`, without compiling the pager's dependency graph.
- **Release builds**: build each package directly with `--locked --release`.
  A preceding debug-profile `cargo check` duplicates dependency compilation;
  lint and test workflows already cover development-profile validation.

## Known caveats

- Every Cargo job installs protoc v29.3 directly ("Install protoc" steps) and
  points `$PROTOC` at it, bypassing the dotslash `bin/protoc` manifest. The
  direct `github.com` download is tried first and `gh-proxy.com` is the
  fallback mirror (deliberate: dodges `protocolbuffers/protobuf` rate-limiting
  and transient runner DNS failures, which bit macos-15-intel before). Local
  dev still uses the dotslash `bin/protoc` manifest unchanged.
- MSI packaging pins cargo-wix 0.3.9 and passes the validated directory
  containing both `candle.exe` and `light.exe` through `--bin-path`.
  WiX binary archives may be flat, whereas cargo-wix appends `bin` to `WIX`.
  MSI failure is optional for release publishing; cancellation still stops it.
- `audit.yml`'s license job is gated by a checked-in `audit-baseline.txt`.
  The first run creates it; later runs fail if the dep license set drifts.
- `nightly.yml`'s `upstream-merge-probe` job fetches the `xai-org/grok-build`
  upstream. It's a canary, not a blocker: failures show as workflow-error
  artifacts but never gate a release.

## Adding a new workflow

1. Pick the smallest concern the workflow owns. Prefer adding a job to an
   existing workflow over creating a new file.
2. Add `concurrency: { group: <name>-${{ github.ref }}, cancel-in-progress: true }`.
3. Pin `permissions: { contents: read }` (or a tighter set).
4. Reuse the standard toolchain / cache pattern from the other workflows.
5. Add a row to the table above and link to the workflow.