# SyncSteward Brand Assets

Generated assets live under [generated](/Users/johndeaton/projects/syncsteward/branding/generated).

Current pack includes:

- app/distribution icon master PNGs
- macOS `AppIcon.iconset` source files
- macOS `SyncSteward.icns`
- GitHub/social preview images in common wide and square formats

Regenerate everything with:

```bash
python3 branding/generate_brand_assets.py
iconutil -c icns apps/syncsteward-macos/Bundle/AppIcon.iconset \
  -o apps/syncsteward-macos/Bundle/SyncSteward.icns
```
