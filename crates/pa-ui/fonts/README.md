Embedded fonts (SIL Open Font License 1.1, see the OFL-*.txt files):

- `InterVariable.ttf` — Inter (variable, full glyph set), the copy shipped by Slint (`i-slint-common/sharedfontique`).
  Same family the web UI loads from `@fontsource-variable/inter`.
- `BricolageGrotesque-SemiBold.ttf`, `BricolageGrotesque-Bold.ttf` — static instances (opsz 24, wdth 100, wght 600/700)
  of `@fontsource-variable/bricolage-grotesque` (latin + latin-ext subsets merged, so Turkish letters are covered),
  made with fontTools `varLib.instancer` + `merge` (see `tools/` in the PR notes). The web UI uses it as `font-display`.
