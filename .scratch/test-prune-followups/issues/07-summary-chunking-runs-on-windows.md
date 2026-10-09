# 07: `tests/summary_chunking.rs` runs on Windows

Status: ready-for-agent

Found by a preservation review during the test prune of 2026-10-09.

`crates/evertranscript-core/tests/summary_chunking.rs` starts with
`#![cfg(unix)]`, so its tests never run on Windows. The prune named several of
them as the replacement for deleted unit tests that did run on both platforms.
Examples (from the lane D ledger):

- `an_item_crediting_the_unnamed_placeholder_is_left_out_and_said_so`, for
  `prompt.rs` `the_operators_own_placeholder_is_left_alone`
- `preselection::a_fresh_install_gets_local_without_choosing_it`, for `knob.rs`
  `a_fresh_install_has_not_chosen_and_says_so`
- the chunked-path tests, for the `fake.rs` tests of the scripted backend

The gate has the same cause as the one DECISIONS Q48 and Q54 removed from
sibling files. One test, `a_summary_being_generated_does_not_hold_up_other_clients`,
builds the endpoint as a file path (`dir.path().join("s")`, `:260`), and
`transport::bind` takes a pipe name on Windows.

## Do

- Use the shared helper: `mod common;` and `common::endpoint(dir.path())` in
  place of the hard-coded path, as `tests/common/mod.rs` describes.
- Remove `#![cfg(unix)]`.
- Check that the Windows CI job runs this file and passes.
- The same gate is still on `summary_quality.rs` and
  `summary_ninety_minutes.rs`. Check whether either still needs it.

## Comments
