# Brand generation tools

The generator uses existing Foundry deployments with Microsoft Entra authentication
through `AzureCliCredential`. It does not provision resources, retrieve API keys or
modify the app. Only the public product brief is sent to the selected Azure resource.

```powershell
python -m pip install -r tools\brand\requirements.txt
$env:AZURE_CONFIG_DIR = '<your isolated Azure CLI profile directory>'
$env:AZBRIDGE_BRAND_ENDPOINT = 'https://<resource>.services.ai.azure.com'
python tools\brand\generate.py --output '<local review directory>' --text-only
python tools\brand\generate.py --output '<local review directory>'
python -m unittest discover -s tools\brand -p test_*.py
```

Choose an existing profile; if authentication expires, use `az login
--use-device-code`. Never commit credentials, tokens or account-specific endpoints.
`prompts.yaml` configures the brief, deployment names and versions. Confirm those
versions with your resource before generation. The pipeline first generates a
validated three-direction/ten-tagline JSON brief, then submits the same prompts
to each configured image model. `--model <deployment>` restricts a resumed run.

Outputs include original PNGs, WebP review copies, and `provenance.json` with
prompts, parameters, dates, versions and source SHA-256 hashes. Existing recorded
files are reused only after checksum verification. Missing/changed recorded
files and unrecorded collisions stop the run instead of silently regenerating.
No content-safety configuration is changed. Refusals, missing image output and
HTTP errors stop generation. HTTP 429 gets bounded backoff; uncertain timeouts
are not automatically retried because they may have incurred a charge.

The standalone board builder embeds all 18 images plus three local OFL fonts:

```powershell
python tools\brand\build_board.py --input '<local review directory>'
```

Before building, put the official unmodified font files in `<review>\fonts` as
`spacegrotesk.ttf`, `ibmplexsans.ttf` and `sora.ttf`, alongside matching
`<identifier>-OFL.txt` files. Sources are the `ofl/spacegrotesk`,
`ofl/ibmplexsans` and `ofl/sora` directories of the official `google/fonts`
repository. Keep downloaded-source URLs and hashes with the review materials.
The board is self-contained: it makes no network requests or model calls.

Concepts are not automatically published. Review direction/model/tagline/typeface
first; redraw the chosen mark as SVG, then review final assets before integrating
them. Raster conversions can strip embedded provenance metadata, so retain the
original PNGs and provenance manifest. Output usage terms are separate from the
repository's code license; do not imply blanket rights or Microsoft endorsement.

## Approved final assets

The chosen recipe is recorded in `final-prompts.yaml`: Offset Span,
MAI-Image-2.6, Space Grotesk and "Outbound paths. Clear control."
It generates eight images: light/dark hero, Connections, Profiles and Gateways.
Resume checks both source hashes and the complete recipe; it does not regenerate
paid outputs if those checks pass.

```powershell
python tools\brand\generate_final.py --output '<local finals directory>'
python tools\brand\build_assets.py --source '<local finals directory>' --concepts '<local review directory>' --output docs\brand
python tools\brand\build_final_board.py --input docs\brand --output '<local finals directory>\azbridge-final-review.html'
python -m unittest discover -s tools\brand -p test_*.py
```

The asset builder uses FontTools to outline Space Grotesk at weight 600 and
Pillow to create icons and banners. It verifies pinned font-source hashes and
original illustration hashes, enforces WebP byte budgets, and records the
complete distributed inventory in `docs/brand/provenance.json`. It only writes
review-stage documentation assets; it never changes the app or deploys them.
Approve the self-contained final board at Gate 2 before application integration.

API references:

- Microsoft Learn: `https://learn.microsoft.com/azure/foundry/openai/how-to/dall-e`
- Microsoft Learn: `https://learn.microsoft.com/azure/foundry/foundry-models/how-to/use-foundry-models-mai-image`
- Microsoft Learn: `https://learn.microsoft.com/azure/foundry/foundry-models/how-to/use-foundry-models-flux`
