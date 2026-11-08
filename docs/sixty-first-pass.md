# Sixty-first pass — structured parsing, the reliability follow-up (v0.5.56)

Verification addendum: #127.

One item, released by Seth 2026-11-07 ("structured
parsing is going to be the key here", after "really want
to get these more reliable"), carrying out the Nov 6
agreement: build the queue first (pass 60 shipped,
v0.5.55), then try structured parsing, with tests to see
if it actually improves the success rate. Nothing else is
in scope. Explicitly NOT in this pass: the tesseract
user-words wordlist (tested no-effect), the profile
long-TTL direction (held), the invalid-payload latch
follow-up (decision pending), the chime volume tweak
(parked), the Oct 27 Voruna Chassis miss (no frames),
and any B-code boundary change.

## The ask

A structured parser as a second voice for relic captions
and part names, measured by A/B admission rates on the
fixture corpus. The pass ships ONLY on a measured
improvement in admission rate with zero false holds.
It measured one: reward-frame admission 13/39 (33.3%)
-> 27/39 (69.2%), burst verdicts 2/5 (40.0%) -> 4/5
(80.0%), pack cells 14/14 throughout, false holds 0 on
both sides. The numbers, per burst, are in §4 below.

## 1. The problem: the fuzzy margin refuses correct reads

The current admission scores a read as one flat string
(confusable-Levenshtein similarity, floor 0.80, margin
0.10). That punishes short strings hardest, and — the
defect this pass actually hit — it cannot tell a correct
read from a 3-edit-different sibling: the margin is
computed over the WHOLE string, so a same-suffix sibling
that differs only in the frame-name token sits close
enough to refuse a perfect read.

Measured on the baseline (v0.5.55, real universes, VM
tesseract 5.3.4):

- "Voruna Prime Neuroptics Blueprint" reads 1.00, but
  "Garuda Prime Neuroptics Blueprint" scores 0.909
  ("Voruna" vs "Garuda" is 3 edits) — margin 0.091 <
  0.10, so all six screen-open Nov 5 Voruna frames refuse.
  The pass-60 burst test passed only because it ran
  against a 7-name slice universe with no Garuda in it;
  under the real parts universe the burst MISSES in
  v0.5.55. (The production Nov 5 credit of the SHORT name
  was the other side of the same coin: production's
  tesseract 5.4.1 dropped "Neuroptics" on one read, the
  short name scored 1.00, and the margin cleared the
  wrong way.)
- "Braton Prime Receiver" reads 1.00, but "Burston
  Prime Receiver" scores 0.909 — margin 0.091, so all
  eight Nov 7 23:26 frames refuse.
- "Sevagoth Prime Systems Blueprint" has no such
  neighbour (runner-up 0.8125), so the three-person set
  admits 7/8 on the baseline already.

The parser fixes the GARBLE class: it knows the shape
of the answer, so it checks each field against its
closed set — "voruna" matches the roster name token
exactly (distance 0), "garuda" is 3 edits away and does
not fit — and exactly one real name fits the read.

It does NOT fix the DROPPED-token class: a read with
the token missing entirely ("Voruna Prime Blueprint"
for the Neuroptics pick) is also a valid parse of a real
short name. That class stays owned by the pass-60 burst
arbitration. The parser NEVER guesses the long name
from a clean short read.

## 2. The grammar (`src/ocr.rs`: `Universe::parse_structured`)

Positional token matching over the universe, counting
EXACTLY how many real names fit the read. One fit ->
admit. Zero -> refuse. Two or more -> refuse. The read
is cleaned as the matcher cleans it (tier label and the
trailing "Relic" stripped), then tried whole and with
every leading-token trim, as `best_match_margin` tries
them — icon bleed prefixes a read the same way. Fits
are unioned across interpretations.

Per-token budgets (candidate token vs read token,
equal token counts required):

- Non-code tokens: confusable-Levenshtein distance
  <= 1, except a token that distinguishes the candidate
  from a strict-subset sibling — the discriminative
  token, "Neuroptics" against "Voruna Prime Blueprint"
  — which may sit at distance <= 2, the "Uroptics"
  case (missing "Ne" is 2 edits).
- Code tokens (candidate matches letter + 1–2 digits):
  the letter matches by the pairwise confusable
  relation (so '!' for 'i', '1' for 'I' still match,
  while 'i' vs 'l' stay distinct — both are real relic
  prefixes); each digit position matches exactly,
  EXCEPT a non-digit garble glyph, which may stand for
  a digit in its class: B for 3 or 8, l/I/|/! for 1 or
  7, s/$ for 5, o for 0, z for 2, g for 6. A clean
  digit is evidence and matches only itself: expanding
  digits would make a clean "C13" refuse against "C18".
  The 3/8 and 1/7 digit mixups live in the garble
  glyphs, not in the digits.

The never-guess guard: a fit that HAS a long sibling
in the universe is refused, not admitted — a clean
short read fits its short name exactly, but the long
sibling is a real name too, and the read cannot tell
them apart. Short names stay owned by the fuzzy
matcher plus the pass-60 burst arbitration; the parser
only ever admits names with no longer token-superset
sibling. (That also covers built forms: "Voruna Prime
Neuroptics" has the "... Blueprint" long sibling, so
the parser never admits a built form either.)

Scope: the parser's distances use the pairwise
confusable relation only. It does NOT touch
`confusable_metric` — pinned by a test asserting
`levenshtein_confusable_metric("ax!", "axi") == 1` —
so the radius decoder's disjointness guarantee is
untouched.

### The B-code finding (judgment call, stated plainly)

The brief's pinned case says "Ax! C1B Relic" must
refuse as genuinely ambiguous between C13 and C18.
That premise assumes Axi C18 exists. It does not:
the baked seed (801 relics, vaulted included,
refreshed in pass 60) holds no `*C18` code in ANY
era, and the wiki's unvaulted list names no Axi C18
either. The counting rule, not the example, governs:
over the real universe exactly one name fits a
B-ending "C1B" read — Axi C13 — so the parser admits
it (and the fuzzy matcher ties 0.857/0.857, C13 vs
C11, margin 0 and refuses — this is the parser's
measured relic win). Where C18 DOES exist (the slice
universe in the tests), the parser refuses the B code
exactly as the brief pins it. And the pre-existing
fuzzy behaviour is unchanged: where C18 exists the
discipline still holds C18 on a B-ending read
(`b_ending_code_token_still_holds_c18` still green),
because the parser never vetoes a fuzzy hold (§3).

The neighbouring shape confirms the counting rule
works: "Lith $1B" fits BOTH Lith S13 (B for 3) and
Lith S18 (B for 8) in the real universe, so the
parser refuses it.

Two more boundaries, pinned in tests rather than
assumed:

- "Axi C99" is a parser refusal (zero fits for a
  nonexistent code) — but at the combined
  `admit_caption` level the fuzzy matcher holds the
  shorter code C9 for a doubled-digit read
  (pre-existing: a doubled 9 reads as C9 at 0.857
  with margin). The parser's refusal does not veto
  that hold.
- Requiem numerals are a known parser limit: the
  per-token <= 1 budget cannot separate
  single-character roman numerals — "Requiem I" fits
  I, Ii and Iii — so the parser refuses all three
  and leaves them to the fuzzy matcher, which holds
  them on margin. "Requiem Eterna" has no such
  neighbours and parses.

## 3. Combination rule: second voice, purely additive

The parser sits ALONGSIDE the current admission
(`admit_caption` / `read_bordered_name`), same pattern
as the synthesis voice in capture — it does not replace
it. The rule, in both wiring points:

- If the fuzzy matcher holds a name, that hold stands.
  The parser is never consulted (pack) or never runs
  (reward): it can neither override nor veto a
  contradicting fuzzy hold.
- If the fuzzy matcher holds nothing, the parser gets
  its turn. A parser proof admits only under the same
  gates the fuzzy path answers to: the shared hold
  predicate (`hold_permitted`, on the proving read),
  and — for relic captions — the token witness
  (`relic_name_witnessed`). A parser-proven name whose
  head the proving read does not witness is not held.
- If the two reads disagree under the parser (each
  proves a different name), the frame refuses, as a
  fuzzy disagreement does.
- The parser's own long-sibling guard means the
  combination cannot mint a short-sibling hold: a
  clean short read is refused by the parser before the
  combination ever sees it.

Wiring: `src/packopen.rs` (`admit_caption`: fuzzy
admit, else parser proof under the hold predicate +
token witness) and `src/reward.rs`
(`read_bordered_name`: only when fuzzy held nothing
on either read, parser proves exactly one distinct
name across the two reads, hold predicate on the
proving read). `select_burst_pick` and the pass-60
arbitration are untouched.

## 4. A/B measurement (harness: `tests/ab_pass61.rs`)

Corpus: Nov 5 Voruna burst (8 frames, committed
fixtures), Axi pack frame + pack-1/2/3 + pack-aya
(14 gated relic cells), three-person reward set
(8 frames), and the three Nov 7 miss bursts, whose
truths were diagnosed from the baseline run's own
reads: 1794112012 "Braton Prime Receiver" (all 8
frames read it at 1.00, margin-refused), 1794112275
"Voruna Prime Chassis Blueprint" (both PSMs' best
match is Chassis on every frame, below the bars),
1794112539 "Vadarya Prime Receiver" (6/8 frames
admit cleanly under the baseline already). The 23:48
Caliban burst from the brief did NOT land — none of
the three recovered dirs is it — so the pinned
Caliban case stays a scripted unit-test case
("Caliban Prime Uroptics Blueprint" -> the long
name; clean "Caliban Prime Blueprint" -> refuse).

Denominator: screen-open frames only. A grab taken
before the screen opens has no pick to admit and is
excluded (exactly one such frame, the Voruna burst's
T+0). Admission rate = admitted-correct / known-true
items; false holds must stay zero.

| Corpus | Baseline (v0.5.55) | With parser (v0.5.56) |
|---|---|---|
| Pack cells | 14/14 (100.0%) | 14/14 (100.0%) |
| Reward frames | 13/39 (33.3%) | 27/39 (69.2%) |
| Burst verdicts | 2/5 (40.0%) | 4/5 (80.0%) |
| False holds | 0 | 0 |

Per burst, reward frames (baseline -> with parser):
Voruna 0/7 -> 6/7 (T+4 still refuses under the
pass-60 short-sibling arbitration: a read held the
short name and the cross-read union witnessed the
long token — the parser never runs there, by design);
three-person 7/8 -> 7/8 (T+0's merged "sevagotnitte"
token has no positional parse); Nov7-2012 0/8 -> 8/8;
Nov7-2275 0/8 -> 0/8 (the "Prime" literal is destroyed
in every read — "Vorundt tie tnassi Blueprint" — so
no grammar parse exists; the parser cannot fix it
and does not guess it); Nov7-2539 6/8 -> 6/8
(unchanged). Burst verdicts: Voruna Miss -> Pick
"Voruna Prime Neuroptics Blueprint" at T+14;
Nov7-2012 Miss -> Pick "Braton Prime Receiver" at
T+14; the rest unchanged (three-person and
Nov7-2539 Pick, Nov7-2275 Miss).

The gate is met: admission improved on known-true
names (frames +35.9 points, verdicts +40 points)
with the false-hold rate at zero on both sides.

Environment note: measured under the VM's tesseract
5.3.4 via the user-space shim
(`~/workspace/.tessroot/usr/bin/tesseract`,
`FRACKER_TESSERACT`); production runs the flatpak's
bundled 5.4.1. The VM's 5.3.4 reads the full long
name on the Nov 5 frames where production's 5.4.1
dropped "Neuroptics" — so the baseline's Voruna 0/7
is a real-universe margin effect measured under 5.3.4,
not a reproduction of the production dropped-token
shape (which the pass-60 arbitration already owns).
The parser's win is margin-shaped, not engine-shaped:
it fixes reads the fuzzy matcher scores correctly
but refuses on margin, whichever engine produced
them.

## Test tally

Full suite green: **511 tests** (481 lib + 15 golden
+ 7 packaging + 6 resident + 1 tray_quit + 1
ab_pass61), 0 failed — 496 baseline + 15 new (8
parser unit tests in `ocr.rs`, 4 scripted
`read_bordered_name` tests in `reward.rs`, 2
`admit_caption` tests in `packopen.rs`, 1 A/B
harness test in `tests/ab_pass61.rs`).
`cargo build --release --locked --offline` green
before the tag.

New tests, by name: `parser_admits_dotted_i_c13_and_
refuses_b_code_and_c99`,
`parser_admits_b_code_where_only_one_sibling_exists`,
`parser_admits_garbled_component_and_refuses_clean_short`,
`parser_refuses_reads_that_break_the_grammar`,
`parser_admits_the_margin_refused_long_name_on_the_real_universe`,
`parser_refuses_requiem_numerals`,
`parser_ignores_leading_icon_bleed_like_the_matcher`,
`parser_never_widens_the_metric`,
`frame_margin_refused_long_read_admits_via_parser`,
`frame_garbled_component_margin_refusal_admits_via_parser`,
`frame_clean_short_read_still_holds_short_on_the_real_universe`,
`frame_parser_disagreement_between_reads_refuses`,
`admit_caption_parser_admits_where_fuzzy_ties`,
`admit_caption_fuzzy_hold_stands_when_parser_refuses`,
`ab_admission_rates_over_the_fixture_corpus`.

No regressions: the pass-60 arbitration behaviour
(burst void rule, cross-read union, pack per-cell
rule) is untouched except where the parser wires in,
and the radius decoder's disjointness tests pass
unchanged — the parser never touches
`confusable_metric`.
