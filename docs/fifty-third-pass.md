# Fifty-third pass — pack-open detection

Verification addendum: #119.

## The ask

Loop stage 1's inflow leak (Seth, queued 2026-10-28,
criteria set the same day;
`PACK_OPEN_DETECTION_QUEUE.md` in the frames workspace):
granted relics from opened packs are named nowhere in
EE.log (content-blind), so pack gains reached the
planner only via a later manual refinement capture.
Detect the reveal screens as they happen: arm on the
pack purchase dialog, grab frames at a 1 s cadence, OCR
the three relic-name captions under the pass-46
admission discipline, and credit proven trios as Intact
— atomically, exactly once per screen. The chime is the
human half of the loop: a proven screen chimes on the
page, so Seth sits on the screen until he hears it.

## 1. Arm (`src/elog.rs`, `src/resident.rs`)

`Event::PackPurchase { path }`: a line
`ThemedDetailedPurchaseDialog.lua: PopulateInfo-><path>`
whose path contains `/BoosterPacks/` — any pack source
(Seth's scope ruling), classified in `classify_inner`
like the other line shapes; a PopulateInfo for anything
else, and any other dialog's line, never arms. Not a
mission end (no regen rides it). The watch loop turns it
into `Trigger::PackScreen(path)`; the resident loop
spawns the one-shot pack watch (config `pack_detect`,
default on, mirroring `reward_detect`). A second dialog
line while a watch is running is ignored — the running
watch is already looking.

## 2. The watch (`src/packopen.rs`, `src/resident.rs`)

`PackWatch` is a pure state machine (instants in,
decisions out); the resident task drives it once per
grab:

- **Cadence**: one portal frame per second. Frames come
  from the live capture stream when one is running (a
  second `FrameTap`, offered the same frames as the
  reward tap), else from the watch's own negotiated
  stream, drained into a shared latest.
- **Session end**: disarm at 15 continuous seconds with
  no pack-classifying frame, or at 60 s total since arm,
  whichever first (Seth: buying happens as a unit —
  unpack screens follow each Yes at sub-second gaps).
  Arm/disarm lines go to capture.log.
- **Exactly once per screen-instance**: an instance's
  trio credits once (later agreeing reads are no-ops);
  the instance ends on an intervening non-pack frame,
  and an identical trio in a later instance credits
  again. A read that conflicts with the stored instance
  (a slot proven both ways with different names — the
  next pack's reveal replaced this one between grabs,
  with no absent frame in between) ends the old instance
  and starts a fresh one with the new read.
- **Diagnostics**: each instance's first frame is saved
  under `capture-debug/packs/`, proven and unproven
  alike.

## 3. Screen gate + caption OCR (`src/packopen.rs`)

Measured on Seth's three fixtures (2560x1440, now
committed as grayscale `fixtures/pack-open/pack-{1,2,3}.png`,
geometry pixel-identical). Cards: columns at x
0.3387 / 0.4512 / 0.5637 of W, width 0.1000, top 0.2583
of H. A frame is a pack screen by structural
conjunction — every column must carry:

- an era icon (bright fraction >= 0.10 at >= 100 in the
  icon zone y 0.30–0.40, inset 15%: fixtures 0.156–0.295);
- caption glyphs (density >= 0.02 at >= 110 in rows
  0.415–0.435: fixtures 0.047–0.056);
- a drop-table column (the fraction of rows in
  y 0.46–0.95 carrying a text line's worth of bright
  pixels within 0.06–0.45: fixtures 0.109–0.160);

and the inter-card gaps must show the dark vendor
backdrop at every sampled row (means <= 25 on the
fixtures; the bar is 40, and reward/gameplay frames
measure >= 53 there). Reward screens fail it (different
card pitch: the pack columns straddle card art, the gap
windows land on panels), gameplay fails it (scene in
the gaps and the drop zone), a blank frame fails it.

**Calibration correction, recorded**: the brief's
nominal caption band (y 0.437–0.458) contains no caption
glyphs on the fixtures — only the dim rule under the
caption and the drop table's diamond. The measured
caption line is y 0.420–0.429 (glyph cores), and the
OCR band is y 0.412–0.440 of H over each card column.

Each caption is read twice (PSM 6 and 7, upscale 4) and
arbitrated like the reward name gate: a name is proven
when a read holds it and no read holds a different one.
A read holds a name under the pass-46 discipline — the
resolver's score (>= 0.80) and margin (>= 0.10) bars,
the shared hold predicate, and `relic_name_witnessed`
(now `pub`, its second caller): every token of the
resolved name witnessed by a distinct read token, in
order, within one confusable edit, with the caption's
trailing "Relic" dropped as furniture before judging.
Unproven is unresolved, never a guess.

## 4. Atomic credit, events, chime

- **Credit** (`packopen::credit_relics`): only when all
  three captions prove. +1 per name into
  `relics[name].tiers[Intact]` — exactly the key form a
  capture-produced Intact relic takes, so planner counts
  are identical; duplicates within one pack credit
  multiply (multiset). Same durability discipline as the
  reward credit (lock, read-modify-write, tmp+rename,
  refuse on an unparseable stock file).
- **Events**: a proven screen appends `pack_open`
  (names in display order; Log line "Relic pack opened —
  A, B, C"). A screen that was seen but left unproven
  when it disappeared or the session ended appends
  `pack_miss` naming the unresolved slot(s) ("unresolved
  slot 2", "unresolved slots 1 and 3"). Both ride the
  pass-51 store and render in the Log tab.
- **Chime**: the newest pack_open rides the Log fragment
  as a hidden `#pack-beacon`, keyed by the event
  identity; a dedicated script (own localStorage
  seen-key, `fracker.packChimeSeen`) plays the shared
  `#credit-chime` element at 0.4 — live arrivals only,
  with the same ~30 s first-sight freshness rule as the
  toast. NO toast for packs, and no chime on a miss.
  A pack credit is a relic credit, not a part credit:
  it never produces a `credit` event, so it can never
  fire the acquisition toast.

## Gates

- All three fixture trios resolve under the discipline
  with the real engine and credit exactly the ground
  truth multisets as Intact (P1 Axi P10 / Meso K9 /
  Lith S19; P2 Neo C11 / Lith V11 / Lith G14; P3
  Axi V14 / Lith A13 / Meso V17) — plus the multiset pin
  (a duplicated name in one pack credits +2).
- Non-pack frames never classify: blank frame, Seth's
  4-player reward screencap, and both 3-player burst
  frames.
- A fixture with one caption made unprovable (slot 2's
  band mirrored — glyph density preserved, text
  witness-less) yields NO credit and one pack_miss
  naming slot 2.
- State-machine pins: 1 s cadence is the caller's;
  disarm at 15 s no-screen and at the 60 s cap; the same
  screen grabbed repeatedly credits once; an identical
  trio after an intervening absence credits twice; a
  conflicting screen replaces the instance and reports
  the old one.
- Full suite green (solo): **431 passed / 0 failed**
  (lib 403, +11 over v0.5.47: packopen 8, elog arm 1,
  events pack pair 1, render pack beacon 1; golden 15;
  packaging 7; resident 5; tray_quit 1).
  `cargo build --release --locked --offline` green
  pre-tag, lock diff = version line only.
- 4-grid reward geometry and all v0.5.47 behaviour
  unchanged (reward paths untouched; the one shared
  change is `relic_name_witnessed` becoming `pub`).

## Live verification owed

Seth buys relic packs on this build: the arm line in
capture.log, each reveal credited + chimed while he
holds the screen, the Log lines for the opens, and a
miss line (not a credit) if a screen is dismissed
before its chime.
