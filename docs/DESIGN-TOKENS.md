# Design tokens

What the desktop and Android clients agree on, and where each one keeps its copy.

Colour needs no spec: `pstr_core::appearance` resolves one `Palette` and both
clients read it, so a flavour is right in both places or wrong in both. The rest
of the token set has no such bridge — Compose cannot consume a Rust constant, and
type and shape do not survive a round trip through `PaletteRecord` — so this file
is the contract, and each side's copy is asserted against it by a test.

| Token | Owner | Android's copy | Pinned by |
|---|---|---|---|
| Colour roles | `pstr_core::appearance::Palette` | `ui/theme/Theme.kt` `schemeOf` | `SchemeRolesTest` |
| Type ramp | `pstr_app::theme::Role` | `ui/theme/Type.kt` | `TypeRampTest` |
| Corner radius | `pstr-app` `ui/mod.rs` | `ui/theme/Shape.kt` | `TypeRampTest` |
| Gradient rule | `Palette::resolve` | — (derived, see below) | `appearance.rs` tests |
| Depth | `pstr_app::theme::{tile_shadow, bar_shadow}` | Material elevation | — |
| Motion | `Context::animate_*` | Compose defaults | — |

## Type

Nine rungs. The desktop names them; Android restates them because Material's own
scale is a different one (`displayLarge` is 57sp, which is a size for a lock
screen and not for anything in a library grid).

| Role | Size | What it is |
|---|---|---|
| Display | 26 | the name of the thing you are looking at, once per page |
| Title | 20 | an empty state, or a page heading with no display line |
| Heading | 18 | the brand in the nav bar; a dialog's heading |
| Section | 17 | a run of content under a rule |
| Subhead | 15 | a row that groups the rows under it — a season header |
| Body | 14 | everything that is prose |
| Label | 13 | the text on a small control |
| Caption | 12 | context rather than content: counts, sizes, errors under a field |
| Micro | 11 | the smallest thing that stays legible — a badge over artwork |

Weight and leading are each side's own: egui has one weight, and Compose derives
line height as 1.35 × size rather than naming it nine more times.

Both clients point their framework's *default* styles at the ramp — egui's five
`TextStyle`s, Compose's fifteen — so a stock widget nobody restyled still lands
on it. Material 1.4 additionally carries an `Emphasized` copy of all fifteen
styles for its expressive components; those are `internal`, cannot be set from
this module, and keep Material's sizes. Nothing in the app draws with them.

## Shape

**Radius 8** for everything either client paints: cards, buttons, the tab pill,
progress bars, dialogs. **4** for a badge over artwork. **16** for an Android
bottom sheet, which has no desktop counterpart.

Two traps, both Android's:

- Material's default shape ramp is 4 / 8 / 12 / 16 / 28, so an unstyled screen
  puts a 28dp sheet beside a 12dp card beside a fully-round button.
- Material's *buttons* read their own shape token (`CornerFull`), not
  `MaterialTheme.shapes`. Handing them a shape is what
  `AccentButton` / `TonalButton` / `EdgedButton` / `QuietButton`
  (`ui/theme/Accent.kt`) are for. Use those, not Material's, at a new call site.

## The accent, and what `gradients` means

The accent is the **only strong colour in the window**. Something wearing it is
something to interact with; nothing wears it for decoration. Neither client
draws an elevation tint (`surfaceTint` is transparent on Android) — a surface
that is faintly accent-coloured because it happens to be raised breaks that rule.

Where the accent is a ramp rather than a flat fill, in both clients:

- the primary button
- the seek bar's spent side
- the progress bar under a tile and under an episode row
- the selected navigation pill *(desktop only — see below)*

Neither client branches on the stored `gradients` flag to decide this.
`Palette::resolve` sets `accent_alt` equal to `accent` when the flag is off, so a
two-stop brush collapses to a flat fill by itself. That is deliberate: a flag
read in two places is a flag that will eventually mean two things, which is
exactly what [B52](BUGS.md#b52--paint-the-accent-as-a-gradient-did-nothing-on-android)
was.

Known divergences, both Android's, both because the component takes a `Color`
and offers no slot to draw into:

- The navigation pill is solid `primary` rather than a ramp.
- A download in flight is solid `tertiary` by choice, not by limitation: it is
  drawn directly under the watch-progress bar of the same row, and two ramps in
  the same colour two pixels apart say nothing.

## Depth and motion

The desktop casts shadows from `theme::tile_shadow` / `theme::bar_shadow`, which
**halve on a light flavour** — the same black that separates a card from a dark
page is a smudge on a near-white one. Android gets Material's elevation, which
already adapts. Neither uses a tint to say "raised".

Transitions on the desktop go through `Context::animate_*`: wall-clock driven,
frame-rate independent, no state held by the caller. Durations in use are 0.05 s
for a press, 0.10–0.12 s for a hover, 0.16 s for the tab pill's travel and 0.22 s
for artwork arriving. Compose's defaults are close enough that nothing here
overrides them.

## Adding a token

Name it in the owning client first, restate it on the other side, and extend the
test that pins the copy. A token that lives in only one place is not a token —
it is a literal with a comment.
