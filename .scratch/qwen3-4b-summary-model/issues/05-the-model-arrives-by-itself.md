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

**Status:** done

- [x] Both, from a pure decision over four facts about the machine
- [x] Size logged before starting; space checked first. The check uses a **checked** add rather than a saturating one — a test found that saturating pins the requirement at the maximum and then compares equal to it, so an impossible need read as satisfied
- [x] Spawned detached and never fatal; resumption is the Downloader's existing behaviour
- [x] `provision_if_fresh` is called by the binary. Constructing a Core fetches nothing, asserted directly rather than inferred
- [x] All nine pass, with **no suppression switch**. This took three attempts and each failure was the test, not the product: the guarantee's own wording is "with updates off **and models downloaded**", and the tests were staging some required models and not others — so the Core correctly fetched what was missing and the tests reported it as a broken guarantee. Staging now reads the required list from the registry, so it cannot go stale the way the hardcoded filename it replaces already had
- [x] Seven unit tests for the decision, and one integration test. **The first version of that test downloaded 3.45 GB from the real mirror and took 200 seconds** — pointing it at the stub through the existing base-URL override brought it to 0.07 s and off the network entirely
- [x] Cached, keyed on the checksums themselves, so the cache cannot serve the wrong file
- [x] Both amended, and the Briefing reworded
- [x] It now says outright that recording locally is not the same as being silent, states the meeting-content promise separately from the network one, and says the first model downloads start on their own
- [x] The local gate is green
