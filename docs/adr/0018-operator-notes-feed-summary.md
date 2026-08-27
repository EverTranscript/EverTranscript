# Operator Notes exist, are always editable, and steer Summary generation

Each Meeting has a markdown Notes pane — Operator-authored writing, jotted during the call or added anytime after. Notes are a first-class entity in SQLite, included in the Markdown mirror, and passed to Summary generation as steering context: what the Operator bothered to write down is the strongest signal of what mattered (Granola's signature behavior, fully local here).

This refines ADR-0009's immutability: the *record* (Transcript, attribution) is immutable; Notes are the Operator's own writing and stay mutable forever. The mirror remains a regenerable projection — Operator content lives in the Notes entity, never in hand-edits to mirror files.
