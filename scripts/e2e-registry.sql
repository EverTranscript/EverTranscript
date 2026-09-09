-- Two Meetings and two voices, shaped to exercise both halves of what the
-- Registry now claims: a first Meeting that has only a detected app (the
-- common case — auto-detected and never titled) and a second that is titled,
-- so a row naming the *later* one would be visibly wrong.
--
-- Alice is heard in both, first in the untitled Teams call. Bob is heard only
-- in the titled one. The Operator is heard in neither, and so has no capture
-- to name at all.

INSERT INTO meetings (id, started_at, ended_at, title, detected_app, created_at, updated_at)
VALUES ('01930000-0000-7000-8000-00000000ea01', '2026-03-04T09:30:00-08:00',
        '2026-03-04T10:05:00-08:00', NULL, 'Microsoft Teams',
        '2026-03-04T09:30:00-08:00', '2026-03-04T10:05:00-08:00'),
       ('01930000-0000-7000-8000-00000000ea02', '2026-05-19T14:00:00-07:00',
        '2026-05-19T14:45:00-07:00', 'Design review', 'Zoom',
        '2026-05-19T14:00:00-07:00', '2026-05-19T14:45:00-07:00');

INSERT INTO speakers (id, display_name, is_operator, voiceprint, voiceprint_model,
                      voiceprint_model_version, confirmed, created_at)
VALUES ('01930000-0000-7000-8000-0000000005a1', 'Alice', 0, x'0102030405060708',
        'wespeaker', '1', 1, '2026-03-04T10:06:00-08:00'),
       ('01930000-0000-7000-8000-0000000005b2', NULL, 0, x'0807060504030201',
        'wespeaker', '1', 0, '2026-05-19T14:46:00-07:00'),
       ('01930000-0000-7000-8000-0000000005c3', NULL, 1, NULL, NULL, NULL, 0,
        '2026-03-04T10:06:00-08:00');

INSERT INTO transcript_segments (id, meeting_id, sequence, channel, start_ms, end_ms,
                                 text, speaker_id, attribution)
VALUES ('01930000-0000-7000-8000-00000000f001', '01930000-0000-7000-8000-00000000ea01',
        1, 'system', 0, 4000, 'Morning — let us start with the rollout.',
        '01930000-0000-7000-8000-0000000005a1', 'clustered'),
       ('01930000-0000-7000-8000-00000000f002', '01930000-0000-7000-8000-00000000ea02',
        1, 'system', 0, 3000, 'The new layout tested well.',
        '01930000-0000-7000-8000-0000000005a1', 'voiceprint'),
       ('01930000-0000-7000-8000-00000000f003', '01930000-0000-7000-8000-00000000ea02',
        2, 'system', 3000, 6500, 'Agreed, ship it behind the flag.',
        '01930000-0000-7000-8000-0000000005b2', 'clustered');
