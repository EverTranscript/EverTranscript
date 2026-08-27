# EverTranscript is open source (Apache-2.0); the repo goes public at M2 quality

The product — Core, Clients, CLI — is open source under Apache-2.0, with the private repo going public once M2 (Auto-Record) reaches dogfood quality. The defining claim is the Closed Boundary, and for a recorder that promises "nothing leaves your machine," source-verifiability is the strongest form that claim can take — a closed-source privacy absolutist carries a burden no entitlement audit fully lifts. The moat is defaults and execution — silent Auto-Record, the always-on Core, working Windows detection — not secrets. **Monetization is deliberately deferred, not solved**: candidates (paid team features, support, paid builds) are post-v1 questions, and anarlog's monetization struggle is the accepted, known risk of this posture.

## Considered options

Open-core (free CE + paid PRO, the Meetily pattern) was rejected because our headline features are the differentiators — gating them guts the story, and not gating them leaves nothing to sell. Proprietary (Granola-posture, revenue-first) was rejected as the hardest possible privacy sell: "trust me, it never phones home" without the source.

## Consequences

- Story 46's verification story gains its strongest clause: read the source.
- The Apache-2.0 LICENSE at the root becomes load-bearing (it began as an initial-commit default); ADR-0028's port-and-attribute NOTICE obligations compose cleanly (Apache→Apache).
- The pre-public window (now → M2) is the last time history can be rewritten; after it, the record of the record is public too.
