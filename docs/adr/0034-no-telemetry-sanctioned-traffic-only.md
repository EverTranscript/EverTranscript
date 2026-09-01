# No telemetry, ever; the wire speaks only Sanctioned Traffic


> **Amended by the Qwen3-4B swap (2026-09-01):** entry two said model downloads happen "at explicit moments". They no longer all do — a fresh install fetches what it needs without being asked. The list is unchanged in length and the wire is still silent beyond it; what changed is that one entry is no longer Operator-initiated, and the Briefing says so. The guarantee's own wording, "with updates off **and models downloaded**, literally zero", is what a steady-state install still satisfies.

The binary contains no analytics or crash-reporting SDK — not opt-in: absent. Crashes write local reports (Core minidumps + logs), surfaced in the UI with manual export (copy, or attach to an issue). All three competitors phone home (Granola: Sentry + Amplitude + Segment + Statsig; anarlog: Sentry + PostHog; Meetily: PostHog); "no analytics SDK exists in the binary" is a sentence they structurally cannot say. The cost — losing crash telemetry from strangers — is accepted for a dogfood-first v1 whose early users are the kind who file issues.

What the product may ever say on the network is **Sanctioned Traffic**, an enumerable, content-free list: (1) the update-feed check, disableable in Settings; (2) model downloads with pinned checksums at explicit moments; (3) the cloud Summary Backend, only when the Operator chose it (ADR-0003 machinery). This resolves the latent contradiction between the "zero network traffic" guarantee test and two ratified mechanisms that are network calls (the updater, ADR-0016; model downloads, ADR-0027/0029/0031): the test rewords to "no traffic beyond Sanctioned Traffic, none of it carrying meeting content — and with updates off and models downloaded, literally zero." The Closed Boundary and Nothing Ambient are untouched: they govern content and input; this governs the wire.

## Considered options

Opt-in crash reporting (one endpoint, off by default) was rejected — the absolutist sentence is worth more than the telemetry. Zero-traffic absolutism with fully-manual updates was rejected as trading away timely security updates.
