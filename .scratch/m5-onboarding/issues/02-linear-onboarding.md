# 02: Linear onboarding — every requirement explained where it is demanded

**What to build:** Story 44's setup flow, so an Operator leaves it armed and is never configuration-prompted mid-meeting.

**Blocked by:** 01.

Status: not started

- [ ] A linear flow: Briefing → permissions → models → History folder → Summary Backend → calendar → done
- [ ] **Each step explains the requirement at the moment it is demanded**, not in a help page. The microphone prompt is the moment to say why the microphone is needed
- [ ] **Permissions**: microphone and system audio on macOS, with the macOS-specific trap named — a system-audio tap is *granted* whether or not the Operator allowed recording, and a refused one delivers silence forever without failing. `audio-check` already records-to-verify rather than asking; onboarding should use it rather than trusting the OS's answer
- [ ] **Models**: what is downloaded, how large, from where, and that it is checksummed. Transcription is required; Summary's model is not (Summary is not an Anchor, ADR-0002)
- [ ] **History folder**: where it is, that it is the complete portable unit, and — stated, not buried — that copies of it contain voice data (ADR-0035)
- [ ] **Summary Backend is not skippable** (ADR-0013): Local badged Recommended, nothing preselected, and the hard warning on choosing Cloud. "Decide later" is a preselection with better manners
- [ ] **Calendar is skippable and Recommended** (ADR-0036), and skipping costs the niceties and nothing else — which the step should say in those terms
- [ ] Every skippable step says what skipping costs, in the step
- [ ] Re-runnable from Settings, because an Operator who skipped something needs a way back that is not reinstalling
