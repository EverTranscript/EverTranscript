# The Backend picker forces an explicit choice; Local is recommended, not preselected


> **Amended by the Qwen3-4B swap (2026-09-01):** the Summary Backend **is** now preselected to Local on a fresh install. The scope of that reversal is deliberately narrow, and the rest of this decision stands: choosing Cloud remains a deliberate act behind the same one-time exfiltration warning, and ADR-0007's recording-start property — nothing is captured before an explicit acknowledgment — is untouched.
>
> What changed underneath is that Local stopped being a gamble. This ADR was written when neither option was obviously right: Local meant a model that might be absent and, when present, produced `None noted.` for plain commitments and invented the timestamps meant to make items checkable. With a **Provisioned Model** that arrives on its own and measures well, an Operator with no basis for the choice is better served by a working configuration than by a disabled Continue.
>
> The cost is real and worth naming: this ADR's broadest claim was that *every configuration the product ever runs traces to an explicit Operator act*. The Core now writes a Backend nobody typed. That is narrowed rather than abandoned — the value is **written** rather than inferred from absence, so a running configuration still traces to a recorded decision, and the one configuration that sends meetings off the machine still traces to a person.

First-run's Summary Backend picker has no preselection — Continue stays disabled until the Operator picks Local or Cloud. Local carries a visible "Recommended" badge; choosing Cloud triggers the hard one-time exfiltration warning.

Every configuration the product ever runs traces to an explicit Operator act — the same property as recording-start (ADR-0007).
