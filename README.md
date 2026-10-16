# Fracker

Warframe session planner: EE.log watcher + public-data planner +
generated HTML checklist, wearing a KDE tray icon. Cracking relics
is basically fracking.

Spec: [BRIEF.md](BRIEF.md) (short summary: [DESIGN.md](DESIGN.md)).
Status: brief complete, build starting 2026-10-15.

## Running Fracker

Fracker is **resident**: launch it and it stays running, watching the
Warframe `EE.log`. When a **new EE.log appears** (a new game session)
it generates a fresh plan automatically — no manual step — and it
regenerates again on mission-end events during the session.

```
fracker                 # resident, KDE tray icon (default)
fracker --headless      # identical watch + report loop, no tray
fracker --demo          # one-shot self-contained review page
fracker --ocr-ingest …  # one-shot inventory OCR bootstrap
```

The tray menu carries:

- **Regenerate now** — force a fresh report run;
- **Open plan page** — open the current report (generating it first if
  none exists yet) via the OpenURI portal;
- **Quit** — exit cleanly.

The disabled line under the menu items (and the icon tooltip) is the
status: last report time and whether the EE.log watch is live.

Reports render to the app data dir:
`$XDG_DATA_HOME/fracker/plan.html` (`~/.local/share/fracker/plan.html`
by default).

`--headless` behaves identically to the tray mode; the tray is pure
presentation and never changes watch/report behavior. It is also how a
user service could run Fracker later.

## Autostart (KDE Plasma)

Fracker is single-instance (an `flock` guard), so a second launch exits
cleanly instead of double-watching the log.

- **Flatpak:** System Settings → Autostart → Add → Application →
  Fracker. (This adds the exported launcher, whose `Exec` Flatpak
  manages.)
- **Unsandboxed / from source:** copy the shipped entry into your
  autostart dir:

  ```
  install -Dm644 flatpak/io.github.sethkinast.Fracker.autostart.desktop \
      ~/.config/autostart/io.github.sethkinast.Fracker.desktop
  ```

The canonical autostart template ships at
`flatpak/io.github.sethkinast.Fracker.autostart.desktop` and is bundled
in the Flatpak under `/app/share/fracker/`.
