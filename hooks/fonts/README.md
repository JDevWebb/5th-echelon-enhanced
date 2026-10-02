# Overlay fonts

The in-game overlay uses IBM Plex Sans (Dear ImGui needs TTF):

- `IBMPlexSans-Regular.ttf`: text
- `IBMPlexSans-SemiBold.ttf`: headings and emphasis

They were converted from the `@fontsource` WOFF2 files (Latin subset) with fontTools, so only Latin characters are included. IBM Plex Sans is under the SIL Open Font License 1.1; the licence text is in `OFL-IBMPlexSans.txt`.

The launcher also uses, unmodified from the official IBM Plex releases (via
google/fonts):

- `IBMPlexSansCondensed-Bold.ttf`: display headings (licence: `OFL-IBMPlexSansCondensed.txt`)
- `IBMPlexMono-Regular.ttf`: numbers and identifiers (licence: `OFL-IBMPlexMono.txt`)

Both are under the SIL Open Font License 1.1, with the Reserved Font Name
"Plex": they're shipped as they are, and a changed copy would need another name.
