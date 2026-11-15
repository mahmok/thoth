# Releasing

Releasing is two manual steps: prepare, then publish. Both are GitHub Actions
workflows; there is nothing to run locally.

The split exists because the version has to be reviewed before the tag exists.

```
push to main ──▶ Prepare release ──▶ release PR ──▶ merge (signed) ──▶ Publish release
```

## 1. Prepare

`.github/workflows/prepare-release.yml` runs `release-plz release-pr`. It runs on
every push to `main`, and on demand from the Actions tab.

It opens or refreshes a pull request titled `chore: release v<version>` containing the
version bump and the new `CHANGELOG.md` entry. Until `1.0.0`, `feat` and breaking
changes bump the minor version and `fix` bumps the patch. All three crates share one
version, so `thoth`, `thoth-core` and `thoth-terminal` never drift apart.

Re-run the workflow if the PR is stale. Editing the PR is fine; editing `Cargo.toml`
by hand is not, because the next run will overwrite it.

## 2. Merge

Merge the release PR **through the GitHub web UI**. The `main` ruleset requires signed
commits, and a web-UI merge is signed by GitHub; a locally pushed merge commit is not,
and will be rejected.

Do not merge anything else after it. `release-plz.toml` sets `release_always = false`,
so the publish step only releases when the tip of `main` is the merge commit of a
release PR.

## 3. Publish

`.github/workflows/publish-release.yml`, dispatched from the Actions tab, runs in order:

1. **verify** — `cargo fmt --check`, `cargo clippy` and `cargo test` on the commit being
   released. A red job means the release is not published. Clippy blocks on compile
   errors only: the workspace lints pedantic and nursery at `warn` and the tree still has
   findings, so it is not yet a `-D warnings` gate.
2. **release** — release-plz creates the `v<version>` tag and the GitHub Release, whose
   body is the new changelog section.
3. **build** — one native runner per target, each running `dist build` from
   `dist-workspace.toml` and uploading its archive and checksum into that release:

   | target | runner |
   | --- | --- |
   | `x86_64-unknown-linux-gnu` | `ubuntu-latest` |
   | `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` |
   | `x86_64-apple-darwin` | `macos-15-intel` |
   | `aarch64-apple-darwin` | `macos-latest` |
   | `x86_64-pc-windows-msvc` | `windows-latest` |
   | `aarch64-pc-windows-msvc` | `windows-11-arm` |

   Each job runs the binary it built (`thoth --version`) before uploading, so a broken
   artifact fails the release instead of shipping.
4. **aggregate checksums** — publishes one `sha256.sum` covering every archive.
5. **verify assets** — fails if any target is missing its archive, its checksum, or
   `sha256.sum` is absent.

The workflow takes an optional `tag` input. Leave it empty: the tag is derived from the
workspace version. The override exists to re-run a release whose uploads failed, and it
is checked against the repository's tags, so it cannot publish a release that
release-plz did not create.

## Required checks and the release PR

CI is meant to be a required status check on `main`. Enable it once this pipeline has run
green at least once, otherwise every PR is unmergeable:

```sh
gh api repos/mahmok/thoth/rulesets/27073222 \
  | jq '.rules += [{
      "type": "required_status_checks",
      "parameters": {
        "strict_required_status_checks_policy": false,
        "required_status_checks": [
          {"context": "fmt"},
          {"context": "test (ubuntu-latest)"},
          {"context": "test (macos-latest)"},
          {"context": "clippy (ubuntu-latest)"},
          {"context": "clippy (windows-latest)"}
        ]
      }
    }] | {name, target, enforcement, conditions, rules, bypass_actors}' \
  | gh api -X PUT repos/mahmok/thoth/rulesets/27073222 --input -
```

The contexts must match the job names in `.github/workflows/ci.yml`.

A release PR is the one PR that will not show those checks: PRs opened by the workflow's
`GITHUB_TOKEN` do not start workflows. Merge it with the administrator bypass the role has
on this ruleset. Nothing unchecked ships anyway, because **Publish release** re-runs
`fmt`, `clippy` and `test` on `main` before tagging.

## What a release looks like

Assets per target: `thoth-<target>.tar.xz` (`.zip` on Windows) beside a `.sha256` file
for that archive, plus one `sha256.sum` covering all of them. Each archive holds the
binary and `LICENSE`.

## Failure modes

- **Publish says no tag exists** — nothing was released, because the release PR was not
  merged, or something else was merged after it. Run Prepare, merge the PR, dispatch
  Publish again.
- **One target fails, the rest uploaded** — fix the cause, then dispatch Publish again
  with the same tag. Uploads overwrite, so this heals a partial release.
- **A build fails on `aarch64-pc-windows-msvc`** — the least-travelled target; it is the
  one to drop from the matrix in `dist-workspace.toml` and
  `.github/workflows/publish-release.yml` if it becomes a recurring problem.

## Not part of this pipeline

No crates.io publishing (the workspace is `publish = false`), no installers, no
dependency-update bot, and no MSRV or licence/advisory job yet.
