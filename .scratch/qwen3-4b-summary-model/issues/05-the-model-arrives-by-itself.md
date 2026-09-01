# 05: The model arrives by itself

**What to build:** A fresh install fetches the Summary model on its own, so an Operator
reaching the feature finds it there rather than discovering a download. Background,
resumable, and never able to hold a recording hostage.

**Provisioning is requested, not implied by construction.** A Core built by a test is not
a fresh install — and the guarantee tests build fresh Cores against isolated Application
Support directories. If construction implied fetching, those Cores would start downloading
inside the test that exists to prove no sockets open, and suppressing that with a test-only
switch would leave the product's strongest claim proven only with the new behaviour off.

**Blocked by:** 03.

**Status:** ready-for-agent

- [ ] The Summary model is fetched automatically on a fresh install; an upgrade that introduces a new required model asks once instead
- [ ] The total size is stated before the fetch begins, and disk space is checked before rather than discovered at ninety percent
- [ ] The fetch is background, resumable across a quit, and never fatal — recording works throughout
- [ ] Provisioning happens because something asked for it; a Core built by a test does not fetch
- [ ] The existing guarantee tests still prove silence, unchanged and without a suppression switch
- [ ] The provisioning decision is tested as a decision, and one integration test proves the wiring against a local stub rather than the internet
- [ ] CI caches the model fetch keyed on its checksum, so a changed model is a different key rather than a stale hit
- [ ] ADR-0002 and ADR-0034 are amended, and the Briefing's network sentence is reworded — it currently says "model downloads you trigger", which this makes false
- [ ] The reworded Briefing separates what leaves the machine from what the machine contacts, because that conflation was observed in practice
- [ ] The local gate is green
