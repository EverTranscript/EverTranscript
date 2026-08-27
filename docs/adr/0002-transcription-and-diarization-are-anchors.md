# Live transcription and Diarization are Anchors; the Backend Knob exists only on Summary

Transcription is the source of truth feeding storage, Diarization, and Summary, so it is permanently local with no Backend selector; Diarization is a local audio model with no cloud-API form. The Knob therefore exists on exactly one feature: Summary.

This deliberately walked back "literally every feature gets the switch" — a cloud ASR path would put the provenance of the entire record in doubt and drag History's foundations across the Closed Boundary.
