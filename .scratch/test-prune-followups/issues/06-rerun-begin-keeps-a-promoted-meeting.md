# 06: Starting a re-run backlog again keeps a Meeting that was promoted to Front

Status: done

Found by a preservation review during the test prune of 2026-10-09. This gap
existed before the prune.

`store::rerun::begin` (`crates/evertranscript-core/src/store/rerun.rs`) deletes the
backlog's own `Back` rows, re-enqueues every Meeting with audio, and keeps a
Meeting as the backlog's when `joined || owned.contains(meeting_id)` (`:233`).

The `owned.contains` term matters for one case only: a Meeting the backlog
owns that someone promoted to `Front`. Its `Front` row is not deleted, so
`enqueue(.., Back)` returns `false` (`diarize_queue.rs:61`). No test reaches this
case. If you remove `|| owned.contains(meeting_id)`, every test still passes,
and the backlog forgets that Meeting: `total` is one short, and the Meeting is
no longer in `diarize_rerun_backlog`.

## Do

- Add a store test in `rerun.rs`: begin a backlog over two Meetings, promote one
  to `Front`, then call `begin` again for a new model.
- Assert that `begin` returns 2, that the promoted Meeting's queue row is still
  `Front`, and that it is still in `diarize_rerun_backlog`.
- Check it: the test fails with `|| owned.contains(meeting_id)` removed.

## Answer

`beginning_again_keeps_a_meeting_somebody_promoted_to_front` in `rerun.rs`
begins a backlog, promotes `b` to `Front`, then begins again. It checks that
`begin` returns 3, that `peek` still gives `b` at `Front`, and that `total` is 3.
Checked: it fails with `|| owned.contains(meeting_id)` removed. A test of what
cancel does with that Meeting was left out on purpose.

## Comments
