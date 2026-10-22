# Worker brief #22 — Fracker twenty-second pass (Axi remap, display slimming, source priority)

Branch: `opencode/twenty-second-pass` (created from
master; check it out). Read FIRST:
`docs/verification-addendum.md` items 66, 67, 68.
Ships as **v0.5.9**.

## §1 Map the "Vanguard" relics to Axi (item 66, resolved)

The baked relic-tables cache names four relics
"Vanguard C1 / E1 / M1 / P1". Seth established (and the
record states): they are legacy AXI relics that do not
currently drop. Fix at the BAKE source, not by
hand-editing the artifact: find where relictables are
baked, map these four to Axi there, regenerate the
cache, and confirm nothing else in the codebase keys
on a "Vanguard" era (era matching, tests, displays).

## §2 Slim the chance text (item 67)

Relic reward rows repeat per-item chance strings like
"Int 25.33% ~4" — identical for every item in a rarity
band, pure noise. Remove the chance/expected figures
from the row display; keep the refinement label
(Int / Rad / etc.). Rarity is carried by color. Apply
everywhere that string pattern renders.

## §3 Source priority + unobtainable treatment (item 68)

The part->relic pick's lowest-rarity choice is correct
and stays. Verify — and where missing, implement — the
source-priority ordering on top of it:

1. owned relics (in captured stock),
2. unvaulted (currently dropping),
3. vaulted but available (Varzia / Prime Resurgence),
4. currently unobtainable (not dropping, not at
   Varzia) — these get a distinct visual treatment:
   strikethrough on the affected relic/part rows.

Downrank items that are even partially unobtainable
(their viable sources include tier 4) in target and
backlog ordering. State in your summary the exact rule
you implemented for "available at Varzia" from the
data the app actually has (worldstate PrimeVaultTraders
inventory is the live source), and the ordering key
before/after for one concrete partially-unobtainable
example from the fixtures.

## Gates and deliverables

- `cargo build` clean, `cargo test` green; bump
  Cargo.toml to **0.5.9**; regenerate
  `docs/demo-plan.html` (rendering changes in §2/§3).
- Logical commits per section; PUSH
  `opencode/twenty-second-pass` and VERIFY with
  `git ls-remote`.
