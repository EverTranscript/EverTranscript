# Live transcription and Diarization are Anchors; the Backend Knob exists only on Summary


> **Amended by the Qwen3-4B swap (2026-09-01):** the Summary model is now a **Provisioned Model** — fetched by default on a fresh install, so the feature is there when an Operator reaches it. Summary remains **not an Anchor**: it keeps its Knob and may run against a cloud Backend. Provisioning is about whether a model arrives on its own; anchoring is about where a feature may run, and the two came apart here for the first time.

Transcription is the source of truth feeding storage, Diarization, and Summary, so it is permanently local with no Backend selector; Diarization is a local audio model with no cloud-API form. The Knob therefore exists on exactly one feature: Summary.

This deliberately walked back "literally every feature gets the switch" — a cloud ASR path would put the provenance of the entire record in doubt and drag History's foundations across the Closed Boundary.
