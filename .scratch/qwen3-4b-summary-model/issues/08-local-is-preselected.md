# 08: Local is preselected

**What to build:** A fresh install has a working Summary configuration without the
Operator having to research a choice they have no basis for. Local is preselected and
recorded, so a stored configuration is always explicit rather than inferred from absence.

Choosing Cloud is unchanged: still a deliberate act, still behind its one-time warning.
What narrows is preselection, and only for the Summary Backend.

**Blocked by:** 05 (preselecting Local means little while the model may never arrive).

**Status:** done

- [x] Written on first start, beside the provisioning request — the two describe the same fresh install
- [x] Written, not inferred. Interpreting absence as Local would have collapsed two states into one, which is the mistake ticket 01 of the previous effort had to go back and fix for titles
- [x] Preselection returns early when any choice exists, tested against a cloud choice specifically
- [x] Untouched — the test that chooses Cloud still has to accept the warning to do so
- [x] Amended, and it owns the cost: the ADR's broadest claim was that every configuration traces to an explicit Operator act, and the Core now writes a Backend nobody typed. Narrowed rather than abandoned — the value is written rather than inferred, and the configuration that sends meetings off the machine still traces to a person
- [x] The local gate is green
