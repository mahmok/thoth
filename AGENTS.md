## Agent skills

### Issue tracker

Issues live in GitHub Issues (`mahmok/thoth`), managed with the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Five canonical roles mapped to `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `GLOSSARY.md` plus `docs/adr/`. See `docs/agents/domain.md`.

### Releasing

Two manual GitHub workflows: prepare the release PR, then publish the tagged release with
binaries. Versions are computed by release-plz, binaries packaged by cargo-dist. See
`docs/releasing.md`.
