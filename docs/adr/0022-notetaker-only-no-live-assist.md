# v1 is a notetaker only; live-assist is out of scope

The Live-Assist cluster — the Teleprompter, Live Chat, and the Stealth overlay — is removed from the product, together with everything that existed only to serve it: the assistant Corpus, local History retrieval (thin RAG), the Moat positioning, and Profiles (with Summary the only LLM feature left, a preset layer over one Knob is pure overhead). The product competes as a full-featured local notetaker — a Granola-grade record and summaries with anarlog/Meetily's locality — not as a notetaker + in-call assistant.

## Consequences

- Retired and removed: ADR-0004 (stealth positioning), ADR-0006 (Live Chat defaults), ADR-0012 (Moat education), ADR-0017 (one app, two clusters). The numbering gaps are intentional.
- Narrowed in place: ADR-0002 (the Knob now exists only on Summary), ADR-0011/0013 (first-run picks the Summary Backend, not a Profile), ADR-0015 (the milestone spine loses the stealth spike and the assist milestones), ADR-0016 (the distribution rationale no longer involves stealth).
- The Closed Boundary (ADR-0001) and Nothing Ambient (ADR-0020) survived this narrowing unchanged — they are properties of the record, not of the assistant. (Nothing Ambient was later narrowed separately by ADR-0023.)
