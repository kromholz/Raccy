# Raccy — 1-bit raster for Flipper Zero

Generated from the canonical `BASE` sprite in `src/render/sprite.rs`, so the
shape is the same Raccy the desktop pet draws — not a redrawing.

| file | what |
|---|---|
| `raccy_32x32.png` | 1-bit PNG, native sprite size |
| `raccy_64x64.png` | 1-bit PNG, 2x nearest-neighbour |
| `raccy.xbm` | 128 bytes, LSB-first, 4 bytes per row |

## Colour to ink

The 32-colour sprite collapses to ink/paper by structure, not by luminance:
outline, hat, trim, fur, mask, jacket, nose and antenna are ink; muzzle, chest,
visor, jacket stripes and the neon accents (tail rings, hat badge) are paper.
That keeps the visor slit, the chest patch and the ringed tail readable at 32 px
instead of flooding the head and body into one black mass.

## Use in a FAP

Drop the PNG in the app's `images/` directory; fbt/ufbt generates the symbol
from the filename:

```c
canvas_draw_icon(canvas, 0, 0, &I_raccy_32x32);
```

Or draw the XBM directly, no build step:

```c
canvas_draw_xbm(canvas, 0, 0, raccy_width, raccy_height, raccy_bits);
```

Regenerate after changing the sprite: `assets/flipper/gen.ps1`.
