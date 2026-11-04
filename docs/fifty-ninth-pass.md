# Fifty-ninth pass — the queue bundle (v0.5.54)

Verification addendum: #125.

This brief supersedes the halted Hunt-only pass-59
draft of 2026-11-04 00:49 (nothing from it landed);
the number was reused for this four-item bundle,
released for building 2026-11-04 02:15. Four items,
all queued that night in the same chat; nothing else
is in scope.

## The ask

1. Hunt page: every gear card's next-relic row sits
   at the card's bottom edge, aligned across cards
   in a grid row regardless of part count.
2. Toast: a small "Cancel" button at the
   part-acquisition toast's bottom-right, cancelling
   THAT SPECIFIC credit.
3. Toast icon: class icons if the data supports
   them, else a refreshed `data/icons.json` covering
   all roster parents — whichever the data supports.
4. Burst: a moved selection voids a stale pick —
   the 2026-11-04 Forma case must not credit Fang
   Prime Handle again.

## 1. Hunt card: the relic row pins to the bottom (`src/render.rs`)

The card (`.g`) is now a flex column, and a growing
spacer (`.gsp`, `flex: 1 1 auto`) is emitted directly
ahead of the footer (`.gf`). The spacer absorbs the
free space, so the footer sits at the card's bottom
edge and footers align across a grid row whether the
card holds 1 part or 5. Position only: the footer
itself is byte-identical (built first, then emitted
after the spacer), and part rows, pips, headers, and
widths are untouched.

Scoping: `.g` is emitted only by `hunt_card_html`
— Backlog renders `.bg`/`.bc` cells and First Crack
renders `.g2` grids, so the rule is Hunt-only by
construction; no other page's markup changes. The
reference screenshot
(`fracker-frames/hunt-relic-row/hunt-cards-20261104.png`:
Venato 2/3, Kestrel 2/4, Alternox 1/4, Daikyu 2/5,
relic rows at different heights) is the before
picture; live visual confirmation remains Seth-side
after install.

Test: `hunt_cards_pin_the_relic_footer_to_the_bottom`
renders a 1-part and a 4-part Varzia-live card and
asserts the flex-column rule, the spacer rule, one
spacer directly ahead of each footer, unchanged
footer text, and 1 + 4 part rows.

## 2. Toast Cancel: undo exactly one credit (`src/events.rs`, `src/server.rs`, `src/render.rs`, `src/resident.rs`)

The toast shell gains a small text button,
`Cancel`, absolutely positioned at its bottom-right
(`#toast .tbtn`). It needs `pointer-events: auto`
because the toast itself is `pointer-events: none`
(so it never blocks clicks); the button re-enables
clicks for itself only. The button is wired to the
showing credit's beacon id (`ts|part`, the toast
script's `currentId`, set from the beacon that fired
the toast) — never to "the latest credit". Clicking
it POSTs `{"id": ...}` to the new `POST
/cancel-credit` route and dismisses the toast either
way. Toast lifetime is unchanged (~6 s auto-dismiss,
`}, 6000);` untouched).

Server side (`CreditCancel`, mirroring the
`UnverifiedToggle` pattern, wired through `serve()`
and the resident with the same Manual regen hook):

- The id parses as `ts|part`; the server finds the
  `credit` record with that exact timestamp and part.
  No match — unknown or stale id — is a safe no-op
  (`200`, `"cancelled": false`, no state change, no
  regen), so a late click can never decrement the
  wrong part. A mismatched pair (one credit's
  timestamp, another part's name) likewise matches
  nothing.
- An already-cancelled id is a no-op too: a second
  Cancel for the same credit never decrements twice
  (guarded by an existing `credit_cancel` record
  naming the id).
- A real cancellation decrements the part's verified
  stock by 1, saturating at 0 (an absent entry stays
  absent; a 0 stays 0, never negative), appends a
  `credit_cancel` event, and fires the regen hook so
  the page reflects the decremented stock.
- An unparseable stock file refuses (`500`, nothing
  logged or regenerated) — the capture-finalize
  discipline: a stock file we cannot read is never
  clobbered.

Event kind name (recorded here per the brief):
`credit_cancel`, rendered in the Log tab as
"Credit cancelled — {part}", carrying `credit_id`
(the cancelled credit's beacon id) so the Log tab
shows credit + cancellation and the record stays
truthful. One same-second caveat, documented in
code: two credits for the same part in the same
second share a beacon id, so only the first is
cancellable by id — but the toast's seen-ids already
dedupe such a pair to a single toast, so the second
never presents a button in practice.

Scope: detection-credit toasts only, by
construction — the pack chime script has no toast
and no button (chime only, as before); the test
pins that the pack script contains no
`cancel-credit` reference.

Tests: `toast_cancel_button_wired_to_the_showing_credit`
(shell, bottom-right + clickable CSS, POST of the
beacon id, unchanged ~6 s lifetime, no pack cancel);
`credit_cancel_roundtrip_and_render` (event schema
and Log line); `cancel_credit_endpoint_undoes_one_credit_by_beacon_id`
(end to end over HTTP: decrement 2 -> 1, log line
with `credit_id`, regen once; repeat id no-op;
mismatched id no-op; unknown id no-op; saturation
at 0 with the log still landing; unparseable body
400).

Deliberately NOT corrected: the 2026-11-04 false
+1 Fang Prime Handle stays in stock — Seth declined
that correction and will reconcile stock himself.
This pass only prevents future false credits (see
§4) and gives him the Cancel button for the next
one.

## 3. Toast icon: parent refresh, not class icons (`data/icons.json`, `src/model.rs`)

### The dispatch-time data check

Fetched the WFCD items dataset through the crate's
own `fetch_items_dataset` machinery (17,935 entries,
78.7 MB, 2026-11-04) and measured it against the
roster (`data/cache/wishlist.json`, 177 parents):

- WFCD parent entries DO carry `imageName`, and the
  67 existing icons agree with WFCD 100% (zero
  mismatches) — the refresh is a pure superset in
  the same scheme (`ParentName` ->
  `https://cdn.warframestat.us/img/<imageName>`).
- WFCD components DO carry a per-part `imageName`
  — but it is generic class art, not per-part art:
  23 distinct component imageNames cover 37 stripped
  component names (e.g. every gun barrel shares
  `GenericGunPrimeBarrel.png`). And the stripped-name
  class key is ambiguous: `Stock` maps to both
  `GenericGunPrimeStock.png` and
  `GenericWeaponPrimeBlade.png`, `Systems` to both
  archwing and warframe art — resolving the class
  needs the parent's category, which the toast's icon
  path (keyed on gear name alone) does not carry.
- The toast's standing rule (addendum #14) is that a
  prime part shows its PARENT item's icon, never a
  generic part icon. Class icons would replace real
  item art with generic silhouettes on all 67
  already-working toasts — a visual downgrade, not
  just a coverage fix.

So the data supports both shapes, but the parent
shape is strictly better here: real art, no
ambiguity, no rule change, and it covers every
creditable parent. Built: the FALLBACK, a refreshed
`data/icons.json` — 67 keys -> 173 keys (+106).

### Coverage of the refresh

- All 163 part-bearing roster parents resolve,
  including the motivating case: Burston Prime ->
  `BurstonPrime.png`, so a Burston Prime Barrel
  credit now shows the Burston Prime art, not the
  monogram "B".
- One documented manual alias: Kavasa Prime has no
  WFCD parent entry, but the entry that owns its
  components is the "Kavasa Prime Kubrow Collar"
  skin, so Kavasa Prime -> `KubrowCollarPrimeA.png`
  (real WFCD art for the item that owns those
  parts, not an invented URL).
- The only roster parents without entries are four
  part-less entries WFCD does not carry either
  (`1200X Kuva`, `2X Forma Blueprint`, `Exilus
  Weapon Adapter Blueprint`, `Forma Blueprint`).
  They carry no parts, so no credit — and therefore
  no toast — can ever key on them.
- The monogram fallback stays for truly unresolvable
  items: no URL is ever invented. The other 10
  part-less roster entries (Requiem words, Ayatan
  stars, …) resolve through their own WFCD entries
  and are included.

Test:
`parent_icons_cover_every_part_bearing_roster_parent`
loads the shipped `data/icons.json` and roster
seed and asserts every part-bearing parent resolves,
Burston Prime resolves to its exact CDN URL, the
only unresolvable parents are the four pinned
part-less ones (so a future roster addition without
an icon fails the test and forces a refresh), and
every value is a bare image filename.

Refresh procedure for the next run: fetch the WFCD
items dataset, join roster parents by exact name
(preferring non-Skin entries), carry `imageName`,
keep the Kavasa alias, and re-run the test above.
No runtime fetch was added — the file stays baked,
per the addendum #51 discipline.

## 4. Burst: a moved selection voids a stale pick (`src/reward.rs`)

The 2026-11-04 live case, code-verified: the burst
credited the LATEST clearing frame, and the name
gate matches the roster parts universe only — so a
frame with Forma selected yields `NameNoMatch` and
never becomes a pick candidate. Seth's roll (Fang
Prime Handle) was pre-selected at screen-open; he
moved selection to Forma, but Fracker credited Fang
Prime Handle, a part he did not receive. Evidence
frame:
`fracker-frames/forma-latch/reward-1793773079-t8.png`
(selection marker on a Forma card). The old doc
comment's rationale ("a border cannot un-happen")
missed the border MOVING to an untracked reward.

The void rule, implemented on per-frame
border-slot evidence only:

- `BurstFrame` gains `border_slot: Option<usize>`,
  populated by the new `detect_pick_slot` (the old
  `detect_pick` delegates to it): `Some(slot)`
  exactly when a border winner cleared the floor +
  margin — for a clearing frame the slot that was
  read, for a `NameNoMatch` frame the slot whose
  name failed the discipline — and `None` when no
  border winner exists. The slot is
  geometry-relative; within one burst the geometry
  is fixed (same screen, same squad size), so slots
  are comparable across the burst's frames.
- In `select_burst_pick`, a pick is VOID (verdict
  becomes `Miss`) if any frame AFTER the pick frame
  carries a border winner on a DIFFERENT slot than
  the pick's slot. The miss line names the move
  ("selection moved from slot … to slot …"), and
  `miss_reason_class` gains the `"selection moved"`
  class so the Log tab's `crack_miss` record stays
  truthful.
- Screen-closed preservation: if no later frame has
  a border at all (later frames are `NotRewardScreen`
  / `NoBorderWinner`, both slot-less), the early pick
  still stands — existing behaviour, still tested.
- Missing evidence never voids: a pick or a later
  frame without slot evidence cannot trigger the
  rule, so a good pick is never voided on noise.
  No threshold was guessed and no threshold-guess
  shipped — the slot comes from the existing
  floor + margin winner gate, the same evidence
  that already decides every border.

Tests, all required, plus guards:

- (a) `burst_moved_selection_voids_the_stale_pick`:
  Fang(clear, slot 0) then Forma(`NameNoMatch`,
  slot 1) -> Miss, no credit, reason class
  `"selection moved"`.
- (b) `burst_screen_close_after_pick_still_credits`:
  Fang(clear, slot 0) then no-border frames -> Fang
  still credits.
- (c)
  `burst_selection_moves_away_and_back_credits_the_final_pick`:
  Fang -> Forma -> Fang with the final Fang frame
  clearing -> Fang credits (latest-wins, and no
  frame after the final pick carries a different
  slot).
- `burst_same_slot_name_miss_does_not_void`: a
  later `NameNoMatch` on the pick's OWN slot is not
  a move — the selection never left, the name just
  failed to read — so the pick credits.
- `burst_missing_slot_evidence_never_voids`:
  pre-pass-59-shaped frames (no slot evidence) never
  void.
- `burst_detect_moved_selection_voids_at_image_level`:
  end to end through `detect_burst` on synthetic
  frames (slot 0 reads a roster part, slot 1 reads
  "Forma") -> Miss.
- (d) all existing burst tests still pass (full
  suite green, see Tests below).

## Explicitly NOT built, per the brief

- Profile long-TTL direction (Seth: "Don't build
  that yet").
- Invalid-payload latch follow-up (Seth's decision
  still pending).
- Chime volume tweak (Seth: "tweak later").
- No stock correction for the 2026-11-04 false +1
  Fang Prime Handle (Seth declined; he reconciles
  stock himself).

## Tests

Full suite green: 478 passed, 0 failed (lib 449,
including the slow OCR stress tests at ~494 s, plus
integration/doc binaries 15 + 7 + 6 + 1). New tests: 1 Hunt fragment test,
1 toast fragment test, 1 event test, 1 cancel
endpoint test, 1 icon coverage test, 6 burst tests
(5 selection-level + 1 image-level).

## Ship

Version 0.5.54: `Cargo.toml` bump plus regenerated
`Cargo.lock` in the same commit (lock diff = the
version line only). `cargo build --release --locked
--offline` green before report. Work commit only —
no merge, tag, or push from this pass (the
coordinator ships).
