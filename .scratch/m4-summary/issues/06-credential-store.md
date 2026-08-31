# 06: API keys live only in the OS credential store

**What to build:** Story 41, on both platforms. M4 is the first milestone that holds a secret at all, which makes the existing guarantee test load-bearing rather than theoretical.

**Blocked by:** nothing.

Status: not started

- [ ] Keys are written to and read from the macOS Keychain and the Windows Credential Manager, and nowhere else
- [ ] **Never in the database, the Mirrors, or the logs** (story 41). `nothing_key_shaped_reaches_the_record_or_the_logs` already asserts this and has never had a key to find — this ticket is what makes it a real test
- [ ] A key is never returned to a Client once stored: the Client asks whether one exists and can replace or clear it. A settings screen that can display a key is a settings screen that can leak one to a screenshot
- [ ] Deleting a key is a first-class act, and switching the Knob to Local does not silently delete it — the Operator may be switching for one meeting
- [ ] The keyring is reachable when the Core runs headless and as a login item, which is how it actually runs. A credential API that only works from a foreground app is a defect that appears the first morning after install
- [ ] Failure to reach the credential store is reported plainly, never worked around by writing the key somewhere else
- [ ] Both platforms, exercised in CI as far as a runner without a login keychain allows — and where it does not allow it, said so rather than skipped silently
