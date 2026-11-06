# 5th Echelon Enhanced: the brand

How the launcher (and the in-game overlay, which follows it) looks and speaks. The Slint launcher
(`launcher-slint/`) is built to these rules; check a new screen against them.

## The mark

Five units stepped back on a diagonal: an echelon formation, the fifth in front in the accent.
`docs/logo.svg` is the master; the launcher draws it (`launcher-slint/ui/brand.slint`,
`EchelonMark`) so it's sharp at every size and takes the theme.

- **Two forms.** On its rounded tile (the app icon, the splash, the side menu, empty states), or
  the units alone (the title bar, anything under 24 px).
- **Sizes.** Never under 16 px. Title bar 18 px (units alone), side menu 40 px (34 px in a short
  window), empty states 56-64 px, splash 112 px.
- **Clear space.** At least a quarter of its size on every side; nothing touches the tile.
- **Colours.** The four units behind in greys that lighten as they come forward on dark
  (`#4a5a62` to `#93a4ad`) and darken on light (`#a9b6b0` to `#4f6159`); the front unit is the
  accent of its background (`#8fd14f` on dark, `#3b7a12` on light). The tile is `#141c20` with a
  `#2b363c` edge on dark, `#e6ebe6` with `#c4cec6` on light. Nothing else recolours it.
- **Don't** stretch it, rotate it, add effects (glow, shadow, gradient), put it on the game's art
  without its tile, or use more than one in a view's content (the side menu's is the window's
  own).
- **Where it is.** The side menu's head, the splash, the title bar, an empty page or list (with
  what to do about it), the guided setup's welcome, What's new, the update banner; in the game,
  the overlay panel's header and the toast when the game starts.

## The name

"5th Echelon Enhanced" in full in running text and the window's title. As a wordmark: "5TH
ECHELON" in IBM Plex Sans Condensed Bold, "ENHANCED" under it in IBM Plex Mono caps, widely
spaced, in the accent (`Wordmark`). In the title bar: "5TH ECHELON ENHANCED" in Plex Mono caps
beside the units, and the release in a small box. Never "5EE" or "5th Echelon" alone where a
player reads it (the server and the network are "5th Echelon": that's the protocol's name).

## Motion

The mark may move in one way only: its units fill in, back to front, then the front one lights
(`EchelonMark { working: true }`): progress the launcher can't measure (the splash, "Checking…").
Screens cross over (fade), the side menu's edge grows, the activity bar's track moves; all of it
short (150-240 ms), eased, and none of it with reduced motion (`Theme.reduced-motion`). Nothing
loops for decoration.

## Colour and type

- **One accent**, night-vision green: `#9be05a` on dark, `#3b7a12` on light. One primary button
  per view. Amber (`#f0b44c` / `#8a5600`) is a warning, red (`#ff7a6b` / `#b3261e`) a failure; a
  colour never carries meaning alone: the words say it too.
- **Tokens only.** Every colour on a screen is a token in `launcher-slint/ui/theme.slint`, with
  a dark and a light value; text tokens meet WCAG AA (4.5:1) on the surfaces they sit on, in both.
- **The stage** (the game's art on Play) is a dark scene in both themes, with its own tokens
  (`stage-*`): the light theme goes light around it, never over it.
- **Type**: IBM Plex Sans for reading (15 px), Plex Sans Condensed Bold for titles (caps), Plex
  Mono caps for HUD labels (11.5 px, spaced). The fonts are in the launcher (OFL), never the
  system's.

## Words

As the launcher already speaks: plain, short, second person ("You're in this match"), the same
word for the same thing everywhere (match, lobby, friends, Quick match, New match), no
technical message without what to do about it.

## The overlay

The panel over the game follows the launcher: the same tokens in both looks (the launcher writes
the look it has when it starts the game, dark or light, to uplay.toml as `OverlayTheme`; "As
the system" is resolved then), the mark on its tile and the wordmark in the panel's header with
the release in its box, titles in Plex Sans Condensed Bold (a pane's in caps), section labels in
Plex Mono caps, the tabs as the side menu's items (the current one on its own ground with the
accent edge and its label in the accent). The panel is a little see-through over the game; imgui
has no blur, and no letter spacing (the wordmark's "ENHANCED" is drawn a letter at a time).
