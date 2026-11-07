# Sixtieth pass — the queue bundle, round 2 (v0.5.55)

Verification addendum: #126.

Five items, all queued Nov 5–6 in the Fracker chat and
released for building 2026-11-06 23:12 ("let's build
everything in queue first"); nothing else is in scope.
Structured parsing (relic/part grammar) is the
FOLLOW-UP after this pass, with tests measuring whether
it actually improves the success rate — explicitly NOT
in this bundle. Also explicitly not in it: the profile
long-TTL direction, the invalid-payload latch
follow-up, the chime volume tweak, the Oct-27 Item 7
Voruna Chassis miss (no burst frames, unreproducible),
and the tesseract user-words wordlist (tested
2026-11-06: no effect on either known failure).

## The ask

1. Hunt footer: never recommend a relic for an owned
   part — the fallback takes the first Need part with
   a deal, and omits the footer when no Need part has
   one.
2. Burst: a dropped token must not credit the short
   sibling — cross-read and burst-level arbitration
   for the Nov 5 Voruna case.
3. Baro pill: flash just the border, not the whole
   pill.
4. Toast: a gold "NEW" line, first, when the credited
   part was needed (any needed copy, 1/2 or 2/2).
5. Pack captions: the dotted-i must not void a cell —
   the ('!','i') confusable pair, plus the Axi C13
   seed correction the investigation surfaced.

## 1. Hunt footer: Need-only (`src/render.rs`)

The fallback branch of `hunt_card_html` now filters
to `PartState::Need` before taking the first deal:
Have and HaveUnverified parts never drive the footer,
and with no Need part with a deal the footer is
omitted entirely, including on fully-ready cards.
The crack-chunk and Varzia branches are untouched.
On the Kompressa Prime card (Blueprint Have, Barrel
Need, Receiver Have) the footer now reads
"▸ Lith K12 · Radiant" instead of the owned
Blueprint's "Lith V11 · Intact" (reference
screenshot: the Nov 5 Kompressa card, 2/3).

Tests: `hunt_footer_recommends_a_needed_part_never_an_owned_one`
(Kompressa shape) and
`fully_ready_hunt_card_renders_no_footer`.

## 2. Burst: short-sibling arbitration (`src/reward.rs`, `src/ocr.rs`)

The Nov 5 defect: the burst credited "Voruna Prime
Blueprint" for a "Voruna Prime Neuroptics Blueprint"
pick, because a dropped "Neuroptics" token lets the
short sibling score 1.0 against the long name's 0.667
and clear MATCH_MARGIN (0.10) by 0.333 — and the
Clause-2 strict-subset guard is one-directional, so
it cannot see the short hold. The fix does not depend
on reproducing the dropped read (the VM's tesseract
5.3.4 reads the full name on every Nov 5 frame
t2..t14; production's flatpak 5.4.1 is the untested
variable and cannot be built on this VM). It uses
the evidence that IS in the burst: psm11's "Uroptics"
on every frame is the long name's discriminative
token, witnessed garbled.

New token-level helpers in `src/ocr.rs`
(`is_strict_token_superset` /
`superset_extra_tokens`, `Universe::long_siblings`,
`Universe::long_sibling_witnessed`): a sibling pair
is two roster names where one's tokens are the
other's with >=1 token inserted in order.

Per-frame (`read_bordered_name`): if the two PSM
reads hold both a short name and its long sibling,
the frame holds the LONG name. If they hold only the
short name, the cross-read union is checked: if the
union of the two reads' tokens witnesses the long
sibling's discriminative token(s) within confusable
distance <=2, the frame does NOT hold the short name
(refuse for that frame). Otherwise the frame holds
the short name.

Burst-level (`select_burst_pick`): the pick is the
latest holding frame, as before — except if any frame
holds the long sibling while the latest holding frame
holds the short sibling, the pick is the LONG name.
True-short preservation: when no frame or read in
the burst contains any evidence of the extra token,
the short name still credits.

Three judgment calls, all inside the never-guess
rule. (a) The burst rule is same-card scoped, not
literally "any frame": a long hold on a
known-different slot never upgrades the pick — that
hold is a different card's name, and crediting it
would be exactly the wrong-credit failure this pass
exists to prevent (the void rule already treats a
different slot as a different card); in production
holding frames carry slot evidence, so the literal
and scoped rules agree whenever the long hold is on
the picked card. Pinned by
`burst_long_hold_on_another_slot_does_not_upgrade_the_pick`.
(b) Two different long siblings held is a miss
("ambiguous"), not a guess. (c) A read token exactly
equal to a token of the held short name never counts
as a witness — it is explained by the short name
itself; required for duplicated-token pairs ("A B"
vs "A B B"), where the short name's own token would
otherwise self-witness on every true short read.
The pass-59 void rule and the screen-closed case are
unchanged, and the score/margin bars are untouched.

Tests (10 new): the (a)–(c) cases above, run against
a new PSM-differentiated stub engine
(`PsmScriptedEngine`) since the dropped read does
not reproduce under the VM's tesseract, plus
`nov5_voruna_burst_credits_the_long_name` — all 8
Nov 5 burst frames committed to
`fixtures/reward-burst/reward-1793928846-t{0,2,4,6,8,10,12,14}.png`
and run through `detect_burst` with the real
tesseract engine, crediting "Voruna Prime
Neuroptics Blueprint". All pre-existing burst tests
pass, including the moved-selection void and
screen-closed cases. Stock note: Seth fixed
stock.json manually for this instance — the fix
prevents future false credits; no stock edit here.

## 3. Baro pill: border-only pulse (`src/render.rs`)

The keyframes are now
`@keyframes baro-pulse{0%,100%{border-color:var(--line)}50%{border-color:var(--gold)}}`
— border-color only, between the line and gold
colours; text and background stay steady. Same 1.6s
ease-in-out infinite timing, still gated on
`plan.baro` present. The #85 test
(`baro_pill_pulses_only_while_the_trader_is_live`)
now asserts the keyframes animate `border-color`
(line ↔ gold) and contain no `opacity`. No other
pill changes.

## 4. Toast: gold NEW line, first, for a needed part (`src/render.rs`, `src/resident.rs`, `src/events.rs`)

The toast shell gains
`<div class="tn" id="toast-new" hidden>NEW</div>`
as the first child of `.tbody` — a third text line,
above the "Relic Cracked" kicker — with CSS `.tn` in
`var(--gold)`. The beacon `#credit-beacon` gains a
`data-new` attribute, and `showToast` toggles the NEW
line from it, hidden by default. Cancel behaviour is
unchanged.

NEW shows iff the credited part's PRE-credit state
was `PartState::Need` — any needed copy, 1/2 or 2/2
(Seth, pinned 2026-11-06 22:10). Duplicates (Have)
and non-goal parts (Unknown) show no NEW.

Credit-path trace: burst pick in `resident.rs`
(`reward_grab`) -> `reward::credit_part` ->
`EventRecord::credit` -> beacon rendered in
`render.rs` `log_html` from `latest_credit`. The new
`pre_credit_was_needed(data_dir, cache_dir,
stock_path, part)` is called BEFORE the stock
increment, mirroring the planner's part-row rule:
scrubbed backlog want, parent gear not
profile-owned, pre-credit verified canonical count
< required (roster + `required.toml`), not
⍻-asserted. The owned set comes from the cached
profile snapshot (the same source the report's
profile fallback prefers) — no live fetch in the
credit path. Unreadable inputs degrade to "not
needed" (no NEW) rather than guessing. Plumbing:
`EventRecord.was_needed: Option<bool>` (serde
default / skip-if-none; old records carry no NEW)
-> beacon `data-new="1"` only when true.

Disclosed micro-race (not a criterion miss): the
pre-credit stock read sits immediately before
`credit_part` but outside `CREDIT_GATE`; a
concurrent cancel-credit for the same part could
theoretically interleave. In practice the only other
part-stock writer is a manual Cancel click, so the
window is negligible.

Tests: `pre_credit_was_needed_mirrors_the_planner_part_row_rule`
(needed 0-held, duplicate, partial 1/2 multi-unit
still NEW, completed 2/2 no NEW, non-goal,
⍻-held), `pre_credit_was_needed_skips_parts_of_owned_gear`,
`toast_shows_gold_new_line_first_only_for_a_needed_credit`
(shell order, gold CSS, JS toggle, beacon `data-new`
on Need / absent on duplicate), plus a `was_needed`
roundtrip assertion in `events.rs`.

## 5. Pack captions: the dotted-i must not void a cell (`src/ocr.rs`, `src/packopen.rs`, `data/cache/relictables.json`)

Two root causes, not one — and the investigation
corrected the brief's premise.

First, the seed was stale. "Axi C13" was NOT in
`data/cache/relictables.json` at all: it is a
brand-new relic DE added in Hotfix 44.1.3 (Oct
26/27, 2026) to un-vault the Braton Prime Receiver,
and the baked seed was last updated Oct 22. Probing
proved it: `best_match_margin("Ax! C13 Relic")`
against the shipped universe returned
`("Axi A13", 0.857, 0.857)` — a tie, margin 0,
refused — and `hold_permitted` returned false
because the name is not in the universe. The frame
itself genuinely displays "Axi C13" (verified by
direct tesseract reads of the caption bands and by
eye: Axi C13 / Lith S19 / Meso Y2). The seed was
refreshed from the upstream Xuerian URL through the
repo's own `fracker --bake-relics` pipeline
(Vanguard remap intact). Net diff: +Axi C13 (and
its 6 item cross-references) and drop-rate
percentages on 14 relics; no rewards, vaulted
flags, or names changed.

Second, the ('!','i') confusable pair. Added
`('!','i') | ('i','!')` to the pairwise
`confusable` predicate in `src/ocr.rs` only. Scope
verified: `confusable_metric` has no `!` component,
so the metric closure is untouched (pinned by a
test asserting
`levenshtein_confusable_metric("ax!","axi") == 1`);
`head_witnessed` uses the metric, and the radius
decoder uses metric distances only. The other
pairwise callers are unaffected in any harmful way
(`lineread`'s blueprint witness strips
non-alphanumerics first; `capture`'s keyword
windows and the `radius_strip_fragment` "prime" cut
only gain a plausible garble match). Full suite and
radius-decoder stress tests pass. Numbers corrected
against the brief: post-fix margin is 0.143, not
0.25 (best 1.0, runner-up "Axi C18"/"Axi C11" at
0.857); pre-fix score was 0.857, not 0.875.

The B-code boundary is unchanged (pre-existing, not
widened): a code token ending in "B" is genuinely
ambiguous between C13 (3 misread as B) and C18 (8
misread as B, and B/8 IS a confusable pair) — the
discipline today holds C18 there, and "Axi C1B" /
"Ax! C1B" still hold C18 at 1.0 after this pass
while clean C13 still holds C13.

Pack arbitration (item 5b) was deliberately LEFT
OUT — it is conditional in the brief, and no
no-guess form exists. First-proven-wins would be a
guess (psm 6/7 have no reliability ordering; a
C13/C18 flip is symmetric). Majority-across-frames
would rework `PackWatch` conflict semantics — a
same-slot different-name IS the screen-changed
signal — to fix a stable OCR error that majority
voting cannot fix (the same misread repeats). And
there is no reproducing case to validate any
arbitration against. The ('!','i') fix plus the
seed refresh are the certain wins; the arbitration
stays open for the structured-parsing follow-up,
which has the grammar to disambiguate codes
properly.

The Aya screen from the same night is untouched by
design (Aya cell has no era icon or caption, gate
fails, skipped without OCR).

Tests: `dotted_i_read_scores_axi_c13_at_one_point_zero`
(asserts the SCORE is exactly 1.0, plus the
metric-scope pin), `pack_1794023098_resolves_all_three_slots`
(new fixture
`fixtures/pack-open/pack-1794023098-3.png`; Axi C13,
Lith S19, Meso Y2 all resolve, no miss),
`b_ending_code_token_still_holds_c18`. The
refinement-corpus pin moved 411 -> 413 (totals
505 -> 507): the two additions are Meso I1 Intact
x3 (the pair breaks the "Meso !1" I1/L1 tie toward
the true 'i' — the fix working as designed) and Axi
C13 Intact x2 (Seth owned 2 on Oct 26); nothing
lost.

## Test tally

Full suite green on the merged branch: **496
tests** (467 lib + 15 golden + 7 packaging + 6
resident + 1 tray_quit), 0 failed — 478 baseline
+ 18 new (10 burst, 5 render, 3 pack).
`cargo build --release --locked --offline` green
before the tag.

Environment note: `/usr/bin/tesseract` vanished
from the VM mid-task (present 04:14, gone by
~05:30; not caused by any agent — no apt/pkill of
binaries). The pack work used the user-space shim
`~/workspace/.tessroot/tesseract-shim` (tesseract
5.3.4, same version) via `FRACKER_TESSERACT`, and
the integration suite ran against it. Also noted:
`fetch::tests::profile_429_locks_both_endpoints`
(a pre-existing stub-server timing test, untouched
by this pass) failed twice under heavy parallel
load during the render work but passed in isolation
and in the final green run — a flake, not a
regression.
