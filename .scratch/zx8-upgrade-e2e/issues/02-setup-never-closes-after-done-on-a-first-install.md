# 02: Setup never closes after "Done" on a first install

Status: ready-for-agent

Priority: medium. Every first install hits it. Closing and reopening the app
gets past it, but nothing tells the Operator to do that.

Found driving a fresh 1.0.1 install on windows-zx8 on 2026-10-10 (Q319). Seen
live on 1.0.1. 1.1.1 and `main` (d6ddd18) have the same code, but I did not
watch a fresh 1.1.1 install.

## What happened

- On step 1 the Briefing was acknowledged, and the Core saved
  `briefingAcknowledged: true`.
- Steps 2 to 6 went through. On step 6, "Done" and "Skip" both did nothing:
  setup stayed on step 6.
- Closing the window and opening the app again showed the main view at once.

## Why

- `App` (`App.tsx:36`) and `Onboarding` (`App.tsx:1824`) each call
  `useBriefing()` (`useCore.ts:598`), so each holds its own copy of the
  Briefing.
- `acknowledge` refreshes only `Onboarding`'s copy.
- The gate at `App.tsx:154` reads `App`'s copy, which was fetched once on
  mount and still says "not acknowledged". So when `onDone` clears
  `showingOnboarding`, the gate still shows setup.

## What to do

- Give the gate and `Onboarding` one source of truth. For example, call
  `useBriefing()` once in `App` and pass `briefing` and `acknowledge` down.
- Add a renderer test that goes through setup from an unacknowledged Briefing
  and asserts the main view appears after "Done". It must fail on `main`.
- No test covers setup today. `scripts/e2e-registry.sh` seeds
  `briefingAcknowledged`, so it skips setup.
