# azbridge identity

**Offset Span** is an open passage made from two opposing, offset architectural
brackets. It represents deliberately connected services, not a closed security
boundary. The product name is **azbridge**, the descriptor is **Relay control
plane**, and the tagline is **Outbound paths. Clear control.**

The identity was selected after comparing three directions across three existing
Azure Foundry image deployments. The final illustrations use **MAI-Image-2.6**.
The logo is hand-drawn SVG, not an automatically traced generated image.

## Logo

| Asset | Use |
| --- | --- |
| `assets/mark-{light,dark,mono}.svg` | Compact product mark; 64-unit master |
| `assets/wordmark-{light,dark,mono}.svg` | Standalone outlined azbridge name |
| `assets/lockup-horizontal-{light,dark,mono}.svg` | Mark, wordmark and descriptor |
| `assets/lockup-stacked-{light,dark,mono}.svg` | Centered lockup for larger placements |

The mark consists of two equal-weight, square-ended brackets on a 64-unit grid.
Each bracket is 8 units thick. Preserve both shapes, their offset, the clear
passage and the original aspect ratio. Do not rotate, round, stretch, add shadows,
enclose the mark in a shield, or close the gap.

Maintain at least **8 units** of clear space around the mark's visible geometry.
Its viewBox already contains this margin; do not crop it away. Give full lockups
at least one bracket stroke of additional space around neighboring content.
Minimum display sizes: mark **16 px** (favicons only), standalone mark **24 px**,
wordmark **120 px wide**, horizontal lockup **240 px wide**, stacked lockup
**180 px wide**. Below lockup minimum sizes, use the mark beside ordinary,
accessible product text rather than shrinking the descriptor.

Use the light or dark edition on its matching background. Monochrome SVGs use
`currentColor` when inlined; set their color explicitly. An SVG loaded through
`<img>` does not inherit the host page's text color. Decorative marks beside the
visible product name should not repeat that name to assistive technology.

## Palette and native Fluent UI

| Color | Value | Artwork role / Fluent relationship |
| --- | --- | --- |
| Relay Blue | `#0078D4` | Primary artwork blue, harmonized with Fluent brand blue |
| Structural Ink | `#182B3A` | Architectural forms and light-edition lettering |
| Chalk | `#F3F5F7` | Light artwork field and dark-edition lettering |
| Signal Ochre | `#B88628` | One restrained alignment detail, not a status color |
| Dark field | `#1F1F1F` | Dark illustration background |
| Dark mark blue | `#479EF5` | Higher-contrast small vector mark on dark backgrounds |

These are **artwork colors**, not replacement application theme tokens. Keep
Fluent's `webLightTheme` / `webDarkTheme`, neutral surfaces, semantic status colors,
focus treatment, accessible controls, square corners and Segoe UI stack.
In the product header the inline mark may use `colorNeutralForeground1` and
`colorBrandForeground1` to follow the active Fluent theme.
Never use ochre instead of Fluent warning or success tokens.

## Typography

The outlined wordmark uses **Space Grotesk SemiBold (600)** with deliberately
compact spacing. It is supplied as paths, so no web-font download is required.
The source font is pinned by URL and SHA-256 in `provenance.json`. Its original
copyright notice and SIL Open Font License 1.1 are in `SpaceGrotesk-OFL.txt`.
The font itself is not redistributed in this asset package.

Application typography remains native Fluent / Segoe UI. Do not apply the
wordmark face to forms, tables, buttons, code editors or body copy.

## Illustrations

| Files in `assets/` | Dimensions | Intended display | Maximum bytes each |
| --- | --- | --- | --- |
| `hero-{light,dark}.webp` | 1200 x 800 | Up to 600 x 400 CSS px | 150,000 |
| `connections-{light,dark}.webp` | 480 x 480 | 160-240 CSS px | 40,000 |
| `profiles-{light,dark}.webp` | 480 x 480 | 160-240 CSS px | 40,000 |
| `gateways-{light,dark}.webp` | 480 x 480 | 160-240 CSS px | 40,000 |

Connections expresses alignment across a gap, Profiles expresses configuration
as aligned structural slabs, and Gateways expresses a host approaching an open
portal. These are editorial metaphors, not protocol diagrams or assurances that
a deployment is configured securely.

Use the matching edition for the active theme. Generated light/dark editions
share a visual grammar but are independently generated compositions, not exact
recolorings. Do not mirror them to imply a different topology. Keep square
artwork frames and generous negative space; avoid decorative borders and filters.
Put guidance and primary actions in real UI beside the artwork, never inside
an image. Use `alt=""` for illustrations that accompany equivalent visible
headings and copy. Set dimensions to avoid layout shifts and lazy-load artwork
below the initial viewport.

## Icons and banners

`favicon.svg` follows the browser's color-scheme preference. `favicon.ico`
contains 16, 32 and 48 px frames; `apple-touch-icon.png` is 180 px.
`icon-192.png` and `icon-512.png` are ordinary app icons.
`icon-maskable-512.png` keeps foreground geometry inside the central mask-safe
circle. `site.webmanifest` references their intended `/brand/` deployment paths.
An icon's background is part of the asset; do not crop to the visible brackets.

The light/dark README banners are self-contained SVGs with outlined lettering
and embedded WebP artwork. `social-preview.png` is 1280 x 640, ready for manual
upload in the repository's social-preview settings. The image is not uploaded
automatically by the application deployment.

The ASCII CLI banner uses the bracket motif, name, descriptor and tagline.
It belongs on interactive foreground **stderr**, not machine-readable stdout.
It must be suppressed for quiet mode, non-TTY stderr, services, TUI and managed
subcommands, and `AZBRIDGE_NO_BANNER=1`.

## Voice

Be precise: name protocols, resources, actions and limitations. Be approachable:
use plain language, active verbs and useful next steps. Be operational:
foreground intent and predictable behavior, not hype or implied endorsement.

Keep the tagline's capitalization and punctuation. Avoid claims such as
"unbreakable", "zero risk" or "Microsoft certified". Explain that opening no
inbound firewall ports is a connection pattern, not a guarantee of security.

## Reproduction and provenance

The recipes are in `tools/brand/prompts.yaml` (concept round) and
`tools/brand/final-prompts.yaml` (approved finals). They exclude existing logos,
Microsoft/Azure branding, people, generated lettering and UI screenshots.
Only the public product brief was sent to the existing Foundry deployments.
No API keys, personal endpoints, source code, customer data or credentials
belong in these recipes or the provenance manifest.

`provenance.json` records the selected direction, font source hashes, model
version, complete final-image prompts and parameters, source PNG hashes,
post-processing, and each distributed asset's checksum and byte size.
Raw generated PNGs remain in the local generation directory. Resizing and WebP
conversion may strip provider-embedded metadata; retain the manifest and raw
originals rather than asserting that derivative files preserve C2PA data.
Generation is not byte-reproducible; rebuilding derivatives from the retained
originals is the reproducible step. Pillow/FontTools versions can affect output.

Use `generate_final.py`, `build_assets.py` and `build_final_board.py` as documented
in `tools/brand/README.md`. The board is a local, self-contained review artifact,
not shipped in the application. Model refusals, invalid outputs and checksum
mismatches stop the build rather than substituting an invented success.

### Attribution and limitations

Space Grotesk is by The Space Grotesk Project Authors; keep the accompanying
OFL notice. The selected illustration deployment is Microsoft's MAI-Image-2.6
through Azure Foundry. Concept comparison also used gpt-image-2.5-flare and
FLUX.2-pro; their images are not distributed as final brand assets.

No trademark clearance, exclusive rights, non-infringement guarantee or Microsoft
endorsement is claimed. The relevant Azure/Foundry service agreement and model
terms still apply; repository licensing does not replace them or the font's OFL.
Account-specific contractual rights have not been audited by this tooling.
