# 07: Homebrew cask and winget

**What to build:** The two install paths story 48 names, so installing never waits on an app store.

**Blocked by:** 06.

Status: not started

- [ ] A Homebrew cask formula pointing at the signed, notarized macOS artifact, with its checksum
- [ ] A winget manifest pointing at the signed Windows installer, with its checksum
- [ ] Both are **generated from the release artifacts** rather than hand-edited, because a hand-maintained checksum is a hash that will eventually be wrong
- [ ] The uninstall path is real on both: what is removed, and what deliberately is not — the History folder is the Operator's and must survive an uninstall (ADR-0035), which is a decision to state rather than a default to inherit
- [ ] **Publishing to the two upstreams needs accounts and a tagged release that does not exist yet.** Prepare the manifests, verify them against a real artifact, and name what only the Operator can do
