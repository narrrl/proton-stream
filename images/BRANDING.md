# Violet / Copper vector branding

Drive, Stream, and Gallery share frontal violet–pink faces, shallow metallic copper backing, and narrow edge highlights. The icons are editable vector artwork with no embedded images or raster effects.

| App | Mark |
| --- | --- |
| Proton Drive for Linux | Open folder with lilac documents |
| Proton Stream | Rounded play prism with lilac and pink currents flowing toward its point |
| Proton Gallery | Stacked photo plates, mountains, and a copper sun |

## Master artwork

Edit `images/logo.svg`. It uses a 256-unit grid with transparent margins, a `mark` group, and reusable linear gradients. The copper backing extends six units up and right. SVG paths, rounded rectangles, circles, strokes, translation, and clip paths (Gallery's photo, Stream's prism) are retained when exporting Android VectorDrawables. Unsupported SVG features cause generation to fail rather than silently change the artwork.

Earlier artwork is preserved in `images/branding-archive/`; `logo-before-current.svg` is the plain prism Stream used before its currents. The new assets replace the active branding without removing previous designs.

## Regeneration

```sh
python scripts/render-branding.py
python scripts/render-branding.py --check
```

Requires Python 3, librsvg (`rsvg-convert`), and ImageMagick (`magick`). Stream's TV banner sets its name in the Inter files bundled in `crates/pstr-app/assets/fonts`, through a fontconfig that sees only them, so the installed fonts cannot change it. Keep the generator identical across the three repositories; every checkout renders independently and requires no sibling checkout. Edit the canonical master rather than generated platform assets.

## Platform treatment

- Desktop SVGs and PNGs retain transparent margins and the exact approved geometry. The same colored artwork works on light and dark surfaces.
- The desktop SVG uses no `clip-path` in its mark: QtSvg, which Qt shells and launchers draw icons with, ignores it, so a clipped shape spills outside the outline. Cut shapes in their own paths; the generator refuses a clipped Drive or Stream mark.
- Android adaptive icons use a deep violet full-bleed plate, lit from the top left like the copper rim, and a separate foreground containing the complete object. The 256-unit canvas maps to the central 64dp of the 108dp adaptive canvas; the painted geometry stays within the central safe circle. Android supplies the outer mask.
- Themed launcher icons and Stream's status-bar notification icon use a white silhouette without colored bevel highlights. Stream's currents sit inside the prism, so its silhouette is the plain play shape.
- iOS uses Android's deep violet plate and the same crop as an Android launcher icon (the visible 72dp window, with the mark filling 64dp of it), so both phones show one icon. Exported opaque at 1024px; the system supplies the corner mask.
- The Android TV banner pairs the play prism with the full Proton Stream name.

Keep full product names in launcher metadata. Application UI typography and photo-content icons are independent of this branding.

## Outputs

Generated outputs include the packaged desktop SVG, the desktop/window/notification PNG, Windows ICO (16, 24, 32, 48, 64, 128, and 256px), Android adaptive and monochrome layers, status-bar notification mark, Android TV banner, iOS source SVG, and opaque 1024px iOS icon. The compatibility entry points `render-ios-icon.sh` and `render-tv-banner.sh` invoke this same generator.
