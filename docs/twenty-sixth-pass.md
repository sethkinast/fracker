# Twenty-sixth pass — header classification, made honest (addendum #83)

Trigger: Seth's v0.5.20 parts capture finalised itself at
~34 s (161 seen / 2 read / 159 skipped, 25 parts, no tray
stop). Diagnosis ran Seth's three uploaded frames through the
production classifier offline. Findings in addendum #83; the
short version is that Parts classification was a coin flip:
"INVENTORY" never survives OCR of the title font (reads back
"VENTORY"), so the class rode on exact "SELL", and one split
read ("SEL L") flipped the run to Other — after which the
pass-24 header cache and the 90-frame idle-stop converted a
one-frame misread into a terminated capture.

Changes (v0.5.21):

1. `ScreenClassifier::classify_text` normalises the header
   text and keywords to squashed uppercase alphanumerics
   (kills split-glyph artefacts) and falls back to a
   confusable-aware fuzzy window match (edit allowance 1,
   or 2 for keywords of 8+ characters — a dropped two-letter
   ligature). Seth's three real header reads are pinned as
   fixtures in `screens_toml_loads_and_scores_the_measured_headers`.
2. Idle accounting: a capture accrues idle only after 3
   consecutive FRESH Other classifications
   (`fresh_other_streak`); any fresh real classification
   resets the streak. Gate:
   `idle_needs_three_consecutive_fresh_other_classifications`.
3. Finalize logs header samples and the debug-frame list on
   every capture (were zero-collection only).

Known trade-off: a perfectly static exit screen no longer
auto-stops (its header never re-reads, so the streak cannot
climb). Animated menus and gameplay still auto-stop; the
tray toggle remains the primary stop.
