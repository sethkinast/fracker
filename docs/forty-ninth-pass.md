# Forty-ninth pass — reward trigger rekey + fewer-card reward geometries

Verification addendum: #115.

## The ask

Two defects found in one evening of live play (2026-10-27), both
with Seth's log evidence in hand:

1. **A phantom arm.** After a true reward screen, the next
   rotation's relic-selection close re-fired
   `ProjectionRewardChoice.lua: Relic rewards initialized`
   ALONE — no `GetVoidProjectionRewards`, no `gets reward`,
   no `Got rewards` (game t=3075.684). The old classifier
   armed on either ProjectionRewardChoice line, and the
   arming flag clears on grab resolution, so the stray init
   re-armed the burst with no screen behind it.
2. **A guaranteed miss on fewer-player screens.** Seth's
   2026-10-27 23:56 burst (3-player, client; frames in
   `fracker-frames/three-person/`) scored [0, 0.192, 0, 0]
   at T+0 and all-zero at T+14 on the production 4-grid:
   the reward row stays CENTRED, so with three players
   the cards sit half a pitch right of the 4-grid and the
   measured rects straddle card edges.

Seth's chosen fix for (1): key the trigger to
`VoidProjections: <his account id> gets reward`. It fires
once per true screen, ~0.15 s after the init line and ~0.3 s
before `Got rewards` (the countdown starts at `Got
rewards`); squadmates produce only `Host/Client got reward
info from` lines; confirmed present client-side by Seth
(2026-10-27 23:56, game t=4151.701, Sevagoth Prime Systems
Blueprint, as client). For (2): keep the 4-person path
first and byte-unchanged; consult centred 3/2/1-card
geometries only on a 4-grid border miss, under identical
gates and thresholds.

## Part 1 — the trigger rekey

**Classifier (`src/elog.rs`).** The two
ProjectionRewardChoice patterns are REMOVED from
`classify_inner` — neither line classifies anything now.
A new event, `Event::RelicReward { roll: String }`, is
produced only by `classify_with_account(line,
account_id)`, matching the exact prefix
`VoidProjections: {account_id} gets reward ` with a
non-empty path. The context-free `classify` delegates
with an empty id, so it can never produce the variant:
the id is the whole signal. `is_mission_end` is
`RelicReward | RelicCrack` — the gets-reward moment
substitutes for the old screen signal, so regeneration
still fires once per true crack, a hair earlier
(~0.3 s before the old `Got rewards` marker).

**Wiring (`src/resident.rs`).** The watch loop — the
classifier's one production call site — now calls
`classify_with_account(&line,
report::PROFILE_ACCOUNT_ID)`: the id comes from the
same constant the profile fetch uses (the brief's
requirement), never a second hardcoded copy.
`Trigger::RewardScreen` carries the roll path (the enum
loses `Copy`; the one Copy-dependent call site takes
`burst[0].clone()`). The main loop partitions the
RewardScreen trigger out of report bursts as before and
hands the roll to `spawn_reward_grab`/`reward_grab`,
whose arm log line names the roll when
`fetch::load_item_names` resolves the path — exact key,
then the StoreItems-stripped dataset form, then the
StoreItems-inserted form (the Varzia manifest
resolution at plan.rs is the precedent). Unresolvable
paths log raw, never guessed: the roll is diagnostic
only, and it is his relic's roll, never his pick. A
recipe/blueprint reward path does NOT resolve against
the item-names data today (verified against the local
copy: no `Recipes` keys at all) — so the arm line
usually carries the raw path until that data grows.
Episode close and coalescing are unchanged (RelicCrack
closes; resolution clears pending).

**Roll naming helper (`reward::roll_display_name`).**
Tries the three dataset shapes above; `None` when the
data does not name it.

## Part 2 — fewer-card geometries

**Generalization (`src/reward.rs`).** The geometry
functions take a row origin in slot units — 0.0 for the
4-grid, 0.5/1.0/1.5 for 3/2/1 cards (`row_origin`) —
and the public 4-grid functions are delegating wrappers
over `slot_rect_at` / `name_band_region_at` /
`border_scores_at` / `border_winner_in` /
`card_present_at` / `is_reward_screen_at`, so the
4-grid path is the original arithmetic, unchanged.
The 1-card presence rule needs exactly one present
card (requiring two would exclude a solo screen by
construction); 2+ keep the ≥2 rule. The name reader
is extracted as `read_bordered_name`: the region is
the only geometry-dependent input, and the two-PSM
arbitration is verbatim.

**`detect_pick` order.** 4-grid presence → 4-grid
border → (on a winner) the unchanged name discipline.
On a 4-grid border miss only, try (0.5, 3), (1.0, 2),
(1.5, 1) in order: presence on that geometry, then
border (same floor, margin, side-line gate), then the
unchanged name discipline. The first geometry yielding
a border winner owns the frame — its name verdict
(credit or refusal) is final. If none yields a winner,
the miss is the 4-grid's own `NoBorderWinner` with the
4-grid scores (miss shape preserved for burst
reporting).

**One refusal is never overridden (found by the
existing test suite, not by inspection).** The first
implementation consulted alternates on ANY 4-grid
border miss, and `ambiguous_border_credits_nothing`
failed: on a 4-person frame with two bordered cards,
the 2-card geometry sees exactly one of them (duo
cards coincide with 4-grid slots 1/2) and "resolves"
an ambiguity the design of record refuses. So an
alternate is consulted only when no slot on the
current geometry clears `BORDER_FLOOR` at all: a
border-strength signal without a unique winner is a
refusal, on every geometry, and detection stops. This
costs genuine fewer-card frames nothing — their
4-grid scores are structurally low (a straddled card
contributes at most one gated side; Seth's real trio
frames measure ≤ 0.192 against the 0.55 floor).

## Verification — fixtures, before and after

**4-person fixtures: identical.**

- `fracker-frames/fixtures/reward-screen.png` (pass-39
  fixture): winner slot 0, continuity in the 0.9s —
  unchanged.
- `fracker-frames/live-relic-sync/reward-1793062149.png`
  (pass-43 live frame, Steflos Prime Receiver): winner
  slot 2 ≥ 0.90, every other slot ≤ 0.10 — unchanged.
- Synthetic 4-person screens (bordered slot, ambiguous
  double border, borderless, gameplay frame) all
  verdict-identical, including the ambiguity refusal
  above.

**3-person frames (Seth's 2026-10-27 23:56 burst):**

- `three-person/reward-1793159728-t0.png` — BEFORE:
  4-grid border scores [0, 0.192, 0, 0], no winner
  (guaranteed miss). AFTER: centred 3-card scores
  [0.996, 0, 0.192], winner slot 0 ≥ 0.90; real-engine
  `detect_pick` returns "Sevagoth Prime Systems
  Blueprint".
- `three-person/reward-1793159728-t14.png` — BEFORE:
  4-grid border scores all zero, no winner. AFTER:
  centred 3-card scores [0.995, 0, 0], winner slot 0
  ≥ 0.90; real-engine `detect_pick` returns "Sevagoth
  Prime Systems Blueprint".

The real-engine verdicts ran against a live tesseract
(5.3.4, user-space extraction at
`workspace/.tessroot` — the sandbox apt install
reported success but left no `/usr/bin/tesseract`;
the tests run through a `/tmp/tesseract-shim` with
`LD_LIBRARY_PATH`/`TESSDATA_PREFIX` and
`FRACKER_TESSERACT`). The skip-if-unavailable pattern
is unchanged: without an engine the tests skip rather
than fail.

## Gates

- `elog::tests::gets_reward_arms_only_for_seths_id`:
  his-id gets-reward (host shape, EE-2 transcription)
  → `RelicReward { roll }`; the same line through
  context-free `classify` → `None`; another id's
  gets-reward → `None`; `Host got reward info from`
  traffic about his id → `None`; client-side shape
  (Seth, 2026-10-27 23:56) → `RelicReward`.
- `elog::tests::transcribed_catalog_markers`: both
  ProjectionRewardChoice lines → `None` now, with and
  without the account id.
- `resident::tests::reward_trigger_keys_on_his_gets_reward_line_only`:
  the fake-init sequence (selection-vote close +
  stray init + a squadmate's grant) arms nothing and
  regenerates nothing; the true sequence (init, his
  grant, Got rewards) arms exactly once with the roll
  attached and regenerates once.
- `resident::tests::mission_end_event_triggers_a_regeneration`:
  re-keyed to the transcribed gets-reward line.
- `reward::tests::three_person_row_detects_via_the_centred_origin`:
  synthetic trio — 4-grid misses (precondition), the
  centred origin detects slot 0's part.
- `reward::tests::one_and_two_person_rows_detect`:
  synthetic duo and solo detect (the duo's cards
  coincide with 4-grid slots 1/2, so that case pins
  the end-to-end result; the solo straddles and
  exercises the 1-card path).
- `reward::tests::borderless_trio_misses_with_the_four_grid_scores`:
  borderless trio → `NoBorderWinner` carrying the
  4-grid scores.
- `reward::tests::three_person_real_frames_resolve_at_the_centred_origin`
  and `three_person_t14_detects_sevagoth_with_the_real_engine`:
  the before/after verdicts above (t0 and t14, border
  level and full OCR).
- `reward::tests::roll_display_name_resolves_dataset_shapes`:
  StoreItems grant path resolves through the stripped
  dataset key; verbatim hit; unresolvable → `None`.
- Full suite green (see addendum #115 for totals);
  census gates untouched; no global `cargo fmt`
  (the codebase is not rustfmt-clean).

## Live verification owed

- Seth cracks a relic on this build: the arm log line
  names his roll, the burst credits the pick, and no
  phantom arm appears after an endless rotation's
  relic-selection close (the 2026-10-27 survival
  scenario).
- Seth runs a 2- or 3-player fissure: the pick credits
  on the centred geometry; a solo fissure likewise.
