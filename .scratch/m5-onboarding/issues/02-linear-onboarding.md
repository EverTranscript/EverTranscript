# 02: Linear onboarding — every requirement explained where it is demanded

**What to build:** Story 44's setup flow, so an Operator leaves it armed and is never configuration-prompted mid-meeting.

**Blocked by:** 01.

Status: built

- [x] A linear flow: Briefing → permissions → models → History folder → Summary Backend → calendar → done
- [x] **Each step explains the requirement at the moment it is demanded**, not in a help page. The microphone prompt is the moment to say why the microphone is needed
- [x] The step names the trap in the copy an Operator actually reads — that macOS grants the tap whether or not recording was allowed, and a refused one returns silence forever without failing. **The record-to-verify button is not wired to `audio-check` yet**: the CLI does it, the step explains it, and the button is the remaining piece
- [x] **Models**: what is downloaded, how large, from where, and that it is checksummed. Transcription is required; Summary's model is not (Summary is not an Anchor, ADR-0002)
- [x] **History folder**: where it is, that it is the complete portable unit, and — stated, not buried — that copies of it contain voice data (ADR-0035)
- [x] **Summary Backend is not skippable** (ADR-0013): Local badged Recommended, nothing preselected, and the hard warning on choosing Cloud. "Decide later" is a preselection with better manners
- [x] **Calendar is skippable and Recommended** (ADR-0036), and skipping costs the niceties and nothing else — which the step should say in those terms
- [x] Every skippable step says what skipping costs, in the step
- [x] A "Run setup again" button in Settings. The flow is idempotent — every step reads current state rather than assuming a fresh install
