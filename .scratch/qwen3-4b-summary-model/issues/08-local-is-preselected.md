# 08: Local is preselected

**What to build:** A fresh install has a working Summary configuration without the
Operator having to research a choice they have no basis for. Local is preselected and
recorded, so a stored configuration is always explicit rather than inferred from absence.

Choosing Cloud is unchanged: still a deliberate act, still behind its one-time warning.
What narrows is preselection, and only for the Summary Backend.

**Blocked by:** 05 (preselecting Local means little while the model may never arrive).

**Status:** ready-for-agent

- [ ] A fresh install has Local as its Summary Backend without the Operator choosing it
- [ ] The choice is written rather than inferred, so "never configured" and "chose Local" stay distinguishable in the record of what happened
- [ ] An Operator who already chose Cloud is never reset by an upgrade
- [ ] Choosing Cloud still requires the one-time warning
- [ ] ADR-0013 is amended with its scope explicit — preselection applies to the Summary Backend only, and the amendment owns that the Core now writes a setting nobody typed
- [ ] The local gate is green
