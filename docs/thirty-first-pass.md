# Thirty-first pass — #87: padding restructure for the NEXT accent

Seth's visual verdict on the pass-28 accent (2026-10-25,
false-color screenshot of the crack queue): STILL WRONG. His
prescribed fix, verbatim: "We need to take the padding off the
container and add it to each row item so the border can float
away from the icon."

## Why the pass-28 construction still rendered wrong

Pass 28 pinned the accent bar to the row's own box
(`.ch.next{position:relative}` + `::before` at left/top/bottom
0, 3px). The geometry was deterministic, but the row box
itself starts at the crack-queue panel's padded content edge:
`.panel` carries `padding:16px 18px`, `.row` carries no
horizontal padding, so the row's left edge — and with it the
bar — sat 18px inside the panel, immediately against the
row's icon column. The bar was registered to the row, but the
row began where the icon effectively begins. Registered is
not the same as separated.

## The restructure (src/render.rs)

Exactly the prescription, scoped to the crack queue:

- The crack-queue section is now `<section class="panel cq">`
  (emitted only by `crack_queue_panel`) with
  `.panel.cq{padding-left:0;padding-right:0}` — the container
  sheds its horizontal padding. The base `.panel` rule is
  untouched, so every other section keeps its 16px/18px inset
  and does not shift.
- Each row item carries the padding instead:
  `.cq .ch{padding-left:18px;padding-right:18px}`. Row
  content — checkbox, icon, text, relic column — keeps its
  exact current visual position; only the row's box grows to
  the panel's full width.
- The panel header keeps its inset via
  `.cq h2{padding-left:18px;padding-right:18px}`: the header
  is not a row item, and nothing but the accent geometry was
  meant to move.
- The accent construction is unchanged from pass 28
  (`::before`, left/top/bottom 0, 3px, gradient kept). With
  the row box now starting at the panel's border edge, the
  bar at left:0 sits in the row's own 18px left padding zone,
  visibly separated from the icon — it floats, per the
  prescription.

Consequences of the prescribed structure, stated plainly:
the rows' `border-top` separators and the NEXT row's
background gradient now span the panel's full width (they
are painted on the row box); vertical padding and rhythm
are unchanged (container keeps its 16px vertical padding,
rows keep their 9px).

## Scope notes

`.ch` rows exist only inside the crack-queue panel, and the
`cq` modifier is emitted only there, so First Crack's other
content, the Routine rows (`.row` without `.ch`, in their own
panels), and every other section are structurally
untouched. No planner, capture, or endpoint changes.

## Tests

The #87 render test now pins the new structure alongside the
pass-28 construction: the `panel cq` section markup, the
container rule shedding horizontal padding, the rows and the
header carrying the 18px, the base `.panel` padding string
unchanged, and the ::before bar pin retained. Full suite:
285 tests green.

## Ship

v0.5.26, commit a4ca3aa, annotated tag `v0.5.26`,
flatpak CI run 40225167567 success. Addendum #87 carries the
resolution. As with pass 28, no live-DOM verification is
available in this environment — the construction follows
Seth's prescription literally; his next live session is the
visual confirmation.
