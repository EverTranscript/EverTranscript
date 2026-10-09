# 04: A test fails when `MATCH_FLOOR` stops being applied

Status: ready-for-agent

Found by a preservation review during the test prune of 2026-10-09. This gap
existed before the prune.

`resolve_with` in `crates/evertranscript-core/src/diarize/cluster.rs` needs three
things to match a cluster to a known Speaker: `clears_floor` (`:281`, against
`MATCH_FLOOR` = 0.62), `clears_margin` and `mutual`. If you replace
`clears_floor` with `true`, the whole suite still passes.

The reason: every test with a score below the floor uses one seed at cosine 0.
With one seed the runner-up score defaults to 0.0, and a best score of 0 then
fails the margin rule first. The floor never decides those cases.

## Do

- Change `a_stranger_becomes_a_new_speaker_rather_than_the_nearest_match`
  (`cluster.rs:1469`) so the stranger's vector is at cosine about 0.5 to its
  one seed. Then the margin passes (0.5 ≥ 0.08), the match is mutual, and only
  the floor makes the result `New`.
- Check it: with `clears_floor` replaced by `true` the test fails, and with the
  code restored it passes.

## Comments
