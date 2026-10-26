# Thirty-ninth pass — reward-pick detection and credit (loop stages 4 + 5)

The crack loop's last two stages. The queue recommends a
fissure and a relic; Seth runs it and picks a reward; until
this pass the app learned nothing from the crack — stock
moved only at the next capture sweep, and the queue could
recommend the same crack again minutes later.

EE.log cannot close that gap by itself (established from
Seth's samples, QUEUE_REVIEW.md): it names only his own
relic's ROLL (`VoidProjections: <guid> gets reward …`), never
the other three rewards, and nothing in the 15 s selection
window records any player's PICK. But the log timestamps the
moment all four rewards exist (`Event::RelicRewardScreen`),
and the choice screen shows the pick directly: the selected
card carries a light frame border. This pass reads the
screen instead of the log: one frame at T+14 s (the screen
lives exactly 15 s; the pick is final in practice by then),
classify it, find the bordered card, OCR its name, credit
+1 to stock, regen. Every failure is a silent MISS — a log
line, no credit — never a wrong credit.

## Trigger + scheduler (scope 1) — `src/resident.rs`, `src/reward.rs`

The watch loop now forwards two new resident triggers:
`Trigger::RewardScreen` on `Event::RelicRewardScreen` and
`Trigger::RewardClosed` on `Event::RelicCrack`. The main
loop partitions them out of report bursts (they never
schedule a report) and drives `reward::RewardArming`, the
coalescing state: "Relic rewards initialized" and "Got
rewards" fire ~0.4 s apart for the SAME screen, so one grab
is armed per episode; re-arming waits for the grab to
resolve or the screen to shut down.

On arming, `spawn_reward_grab` starts the T+14 clock and
pays portal startup immediately:

- If an inventory capture session is live, its stream is
  reused through `reward::FrameTap` — a `TapSource`
  decorator on the capture worker's frame source offers
  every frame to the tap, which retains clones only while
  armed, each stamped with its arrival time. At T+14 the
  grab takes the freshest frame no older than 3 s (a
  session that stopped mid-episode leaves a stale frame
  whose border may not be final; staleness is a miss).
- Otherwise the grab negotiates its own ScreenCast stream
  at T+0 (restore token first, per the capture flow) and
  drains frames into "latest" on a thread until the
  deadline. Negotiation or stream failure is a miss.

Config: `reward_detect` (TOML bool, default on) threaded
through `ResidentOptions`; the headless resident honours it
identically.

## Classification gate (scope 2) — `reward::is_reward_screen`

Structural, not textual. The "VOID FISSURE / REWARDS"
banner was measured and rejected as the gate: its glyphs
are thin and letterspaced and do not OCR reliably even at
capture resolution (fixture header read: `"= < 5"`). The
card row is unmistakable instead: `card_present` requires,
per slot, a dark translucent panel interior (mean ≤ 130;
fixture 52–71) AND real text in the name band (≥ 120 bright
(≥130) pixels at fixture area, scaled by frame area; fixture
227–483). At least two present cards classify the frame as
the reward screen. Gameplay and extraction frames do not
carry the row; a negative control (the inventory browse
strip) produces no credit. The border and name gates below
protect the credit regardless — classification is the
coarse gate, not the safety case.

## Border detection (scope 3) — `reward::border_scores`

Geometry measured on `fixtures/reward-screen.png`
(1769×996) and expressed as frame fractions: card slot 0 at
x 0.34200, pitch 0.07971, card width 0.07744, top 0.31627,
height 0.13705 (top frame line y=315, bottom y=451.5 — an
earlier taller estimate put the bottom edge on the rarity
bar below the panel and measurably diluted the signal).
Per slot, each perimeter edge is scored by max-projection
over a small window straddling the card edge (the frame
line is 1–2 px); the score is the fraction of perimeter
samples ≥ 100. The winner must clear an absolute floor
(0.55) AND a margin over the runner-up (0.30). On the
fixture: continuities 0.956 / 0.221 / 0.000 / 0.035 — the
selected card is not close. Two equally bordered cards, or
none, are a miss.

## Name OCR (scope 4) — bordered card only

The bordered card's name band (card fractions x 0.02–0.98,
y 0.70–1.0 — the bottom-anchored text zone; fixture text at
y 420–450 of the 315–451.5 card) is read twice through the
existing engine's `read_text`, at PSM 6 and PSM 11.
Measured on the fixture and on synthetic screens, neither
mode dominates: PSM 11 dropped the synthetic screen's first
line entirely; PSM 6 garbled the fixture's second card
worst. Arbitration mirrors the grid paths' doctrine: a read
HOLDS a name when `best_match_margin` clears score ≥ 0.80
(the relic floor) and margin ≥ `MATCH_MARGIN` over the
runner-up; the name is credited when the holds agree (one
or both), and two holds naming DIFFERENT parts are an
active contradiction — a miss. Other cards' names are not
read (they belong to the P2 confirm surface, not built).

## Mark (scope 5) — `reward::credit_part`

On a detection, +1 of the canonical part name lands in
stock.json's verified `parts`, with the /unverified
mutation's durability discipline: a serialising gate,
read-modify-write, tmp+rename via `StockState::write_to`,
and refusal on an unparseable stock file (never clobber).
A verified credit also prunes the part's `unverified`
entry, matching capture finalize's treatment of asserted
parts the capture verifies. The existing regen trigger
(`Trigger::Manual`) fires after the write. Observability:
the capture log gains `reward` lines for the arm, the
credited part with its new count, and every miss with its
reason; the grabbed frame is saved to
`capture-debug/rewards/reward-<epoch>.png` before detection
runs, on successes and misses alike. No relic is
decremented (the screen does not show which relic produced
a card — established) and ticks are untouched
(presentational only — established).

## Gate

- Full suite: **336 passed / 0 failed** across all targets
  on the shipped tree (lib 308, including the 6 new tests
  and pass 38's 9; integration 15 + 7 + 5 + 1; the
  pre-rebase tree measured 327). The settled-census gate
  is untouched — this pass adds no recognition changes to
  the grid paths.
- New tests (`src/reward.rs`): a synthetic reward screen
  (four named cards, one bordered) detects and credits
  end-to-end against a temp stock file, including the
  unverified prune and a second increment; a bright
  gameplay frame credits nothing; two bordered cards (and
  none) credit nothing; a double trigger arms exactly one
  grab; credit refuses an unparseable stock file and
  leaves it byte-identical. A sixth test runs the REAL
  tesseract engine over a synthetic screen whose names are
  rendered with the vendored game font (crate::ttf) and
  detects the bordered card's part with no scripted
  answers; it skips when tesseract is unavailable, per the
  house fixture-test pattern.
- Manual fixture verification (the brief's check), run
  against `~/workspace/fracker-frames/fixtures/reward-screen.png`
  with tesseract 5.3.4: classification true (4/4 cards
  present), border continuities 0.956/0.221/0.000/0.035 →
  slot 0, slot 0's read `"- ual Kamas Prime B lueprint"`
  matched **Dual Kamas Prime Blueprint** at 0.923
  (runner-up 0.808; the D is partly clipped by the kamas
  icon — at native 2560×1440 capture resolution the margin
  should widen). DETECT line: `Dual Kamas Prime Blueprint`
  — the brief's expected card and name. Re-run after the
  rebase onto v0.5.32: identical output.
- `cargo build --release --locked --offline` runs at ship
  time (post-rebase), per the standing release gate.

## Deviations and notes

- Classification is structural (card-row presence), not the
  banner OCR the brief listed first among equals; the
  brief left the method to the implementer, and the banner
  measurably does not read. Recorded above with numbers.
- Card geometry assumes the four-card row of a full squad.
  With fewer rewards the game recentres the row, so fixed
  slots misalign and the grab misses (safe, never wrong).
  The design centre is the squad radshare case; sub-four
  layouts are unhandled, deliberately.
- The one fixture margin worth watching: slot 0's name
  match cleared `MATCH_MARGIN` by 0.015 on a 1769-wide
  screencap. Native captures are 2560 wide; live
  verification will confirm the margin there.
- Relic decrement remains open by design (no source-relic
  attribution on the screen). The P2 confirm surface
  (showing all four read names for a one-tap correction)
  is not built.

## Open items

- Live verification — a real crack on Seth's machine — is
  post-ship by definition; the first `reward` lines in a
  live capture.log are the evidence to collect.
- Pass 38 (v0.5.32) landed first, as planned; this pass
  was rebased onto it before merging (only the addendum
  tail conflicted; this entry is 104, after pass 38's
  103).
