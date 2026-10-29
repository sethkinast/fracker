# Fifty-sixth pass — ditch the crack-screen MissionEnd triggers

Verification addendum: #122.

## The ask

Seth's settled criteria, 2026-10-28 (after the
double-requesting walkthrough; this brief supersedes
the 5-second dedupe-window draft, which is NOT built):
remove MissionEnd from RelicReward and RelicCrack, so
mission-end regens come only from the
proven-completion marker. Scope is MissionEnd only:
FileChanged, SessionStart, and Manual — especially the
post-credit regen — are untouched, and all completion
bookkeeping (alert events, completions list pushes) is
unchanged.

## 1. The removal (`src/resident.rs`, `src/elog.rs`)

The watch loop used to send `Trigger::MissionEnd` for
any event whose `Event::is_mission_end()` was true —
that is, for his gets-reward grant line
(`VoidProjections: <his id> gets reward …`) and for
the DVRCAftermath close line. Both sends are gone:

- The `is_mission_end()` block in the watch loop is
  deleted, and `Event::is_mission_end()` itself is
  deleted with it (it had no other caller; leaving a
  method whose answer was now false for every variant
  would have been a trap for the next reader).
- The `Trigger::MissionEnd` doc now says what it
  means: a proven mission completion — the MissionLog
  success marker for an armed mission — and that the
  crack-screen lines are arming/closing signals, never
  report triggers.
- The proven-completion send further down the same
  loop is untouched: when `missions.feed` resolves on
  `GiveMissionRewards. success=true` or
  `EndOfMatch.lua: Mission Succeeded`, the loop still
  pushes the completion (plus the alert event, when the
  identity is an alert) and still sends
  `Trigger::MissionEnd`.

Untouched, line for line:

- Reward burst arming: a RelicReward still sends
  `Trigger::RewardScreen(roll)`, and the resident loop
  still partitions it out of report bursts and arms one
  burst per episode through `RewardArming`.
- Episode closing: a RelicCrack still sends
  `Trigger::RewardClosed`, and the loop still calls
  `reward_arming.on_closed()` on it.
- The credit regen: `reward_grab` still sends
  `Trigger::Manual` after a successful credit, and the
  loop still schedules a report on any non-reward
  trigger, Manual included.
- FileChanged and SessionStart sends, and the
  completion bookkeeping above, are unchanged.

## 2. The resulting per-crack regen story

Two runs, each doing real work, and no third run
hiding between them:

1. Success marker → mission-end update. The
   EndOfMatch / GiveMissionRewards marker proves the
   armed mission, and its MissionEnd regen refreshes
   completions and fissure state (including alert
   strikes).
2. Credit Manual → stock update. The burst resolves
   (detection at T+14 at the latest), the pick is
   credited to stock.json, and the Manual regen
   refreshes the page with the new stock.

Before this pass the crack screen itself fired a
regen on the grant line (and again on the aftermath
line), before the mission was proven complete — the
double-requesting Seth walked through. That regen is
what is removed; nothing replaces it, because the two
runs above already cover the crack's two real state
changes.

## 3. The mid-mission-start edge case

A resident started mid-mission never sees the
MissionInfo arm, so `MissionLog::feed` drops the
success marker ("a completion with no armed mission
is dropped — never inferred from proximity alone")
and run (1) above never fires. Nothing is lost: the
crack screen still arms the burst from the grant line
(which needs no armed mission — it keys on his
account id alone), the burst still detects and
credits, and the credit Manual still fires run (2),
so stock still updates. Only the
completions/fissure-state half is skipped, and that
half had nothing proven to record anyway.

## 4. Explicitly not built

- No 5-second dedupe window (superseded by the
  removal; there is now nothing left to dedupe —
  MissionEnd fires once per proven completion, and
  proven completions cannot double-fire because the
  armed mission is consumed on resolve).
- No incremental/delta planner. Full server-side
  regen stays; the browser update is already
  fragment-diffed, and per-run cost is local CPU once
  fetches are TTL-cached.
- No profile long-TTL direction (held; see
  DE_PROFILE_CACHING_DIRECTION.md — not built until
  Seth releases it).

## 5. Gates

- Gets-reward line: exactly one `RewardScreen` arm,
  zero `MissionEnd` (`resident::tests::
  gets_reward_line_produces_no_report_run`).
- DVRCAftermath line: exactly one `RewardClosed`,
  zero `MissionEnd` (`resident::tests::
  dvrcaftermath_line_produces_no_report_run`).
- Proven completion: an armed MissionInfo block plus
  its `Mission Succeeded` marker yields exactly one
  `MissionEnd` (`resident::tests::
  proven_completion_marker_still_regenerates`).
- Two genuine missions, each armed and confirmed in
  its own watch tick, yield exactly two `MissionEnd`
  triggers — one per completion, never coalesced,
  never dropped (`resident::tests::
  genuine_missions_apart_produce_one_run_each`).
- Credit Manual: the tray Regenerate verb — the same
  `Trigger::Manual` the post-credit path sends, through
  the same loop scheduling — produces a real report
  run (`tests/resident.rs::
  manual_trigger_still_produces_a_report_run`). The
  credit send site itself (`reward_grab`, on a
  successful `credit_part`) is unchanged code, and the
  loop's Manual scheduling is unchanged code; the test
  proves the shared trigger path end to end rather
  than re-proving two untouched lines.
- Burst arming/closing unchanged: the grant test
  above still asserts exactly one arm per true screen
  (and now asserts no regen beside it); the fake-init
  sequence and a squadmate's grant still arm nothing;
  `reward::tests::double_trigger_arms_exactly_one_grab`
  still proves one grab per episode with
  close-then-re-arm; classification of both
  crack-screen events is preserved
  (`elog::tests::crack_screen_events_still_classify`).
- Full suite green (solo): 457 passed / 0 failed
  (lib 428, +3 over v0.5.50; golden 15; packaging 7;
  resident 6, +1; tray_quit 1); `cargo build
  --release --locked --offline` green pre-tag, lock
  diff = version line only. (Process note:
  `cargo generate-lockfile` re-resolved schannel
  0.1.29 → 0.1.30 from the offline index; that hunk
  was reverted by hand so the lock diff is exactly
  the fracker version line.) All v0.5.50 behaviour
  otherwise unchanged.

## Live verification owed

Seth plays on this build and confirms: a crack now
produces exactly the two expected page updates (the
mission-end update on the success marker, the stock
update on the credit) and no earlier flicker on the
reward screen; a resident started mid-mission still
updates stock after a crack's credit; the installed
app reports 0.5.51 (superseding the 0.5.50 install
check).
