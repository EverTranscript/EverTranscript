# Cloud provider presets are curated and labeled, never gated

The curated presets include only providers whose API terms are no-training-by-default, and each preset carries a data-handling label — trains on inputs? retention window? ZDR available? — verified at release time. The custom base-URL field stays fully open, labeled "unknown endpoint — your rules." Labels are information, never gates: the product cannot verify provider-side retention, so a ZDR-only gate would be false hardness dressed as a guarantee, while blocking the Operator's own explicit choice (BYO-LLM, expose every knob).
