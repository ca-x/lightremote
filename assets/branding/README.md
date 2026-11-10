# roc-remote branding

The eagle logo was supplied and approved by the project owner on 2026-11-10.
`logo.png` preserves the uploaded original. `icon.png`, `icon.ico` and the Web
favicon are size-only derivatives, generated with ImageMagick; no reference
project artwork was copied.

On 2026-11-10, the logo was replaced with the owner's newly uploaded PNG
(`a6a9b8b0-881e-452d-bfa8-6cbd986a4869.png`). `logo.png` preserves its original
bytes, verified with `cmp`. The source is 1254×1254; the regenerated PNG icon
is 256×256, the ICO contains 256, 128, 64, 48, 32 and 16 pixel sizes, and the
Web favicon is 32×32. All retain transparency. Only resizing and derivative
metadata stripping were applied: no background removal or artwork changes.

SHA-256 digests for this replacement:

```text
0428a6d5646d68019e792605491da97d120e3d29a57af07bf5c51de3a7a4acb1  assets/branding/logo.png
7ffec0961fa5c02abf17a36e253c57de9f1431c9b0ac012de7ce67048648d02c  assets/branding/icon.png
db3c42256da1733fbfc1298075509c4e8609fdd4164ddb0ca3e7b60990f588dc  assets/branding/icon.ico
a22fca2997ec36bbbecf1c11392a3fe51443b57e705d6f7486f19d3cb45e4a7f  web/public/favicon.png
```

The native controller embeds the PNG in its header. On Windows, keep the
archive's `branding/icon.ico` alongside the executable: NSH uses it for the
window, taskbar and Alt+Tab. The raw executable is not an installer and does
not contain an embedded Explorer icon resource. macOS archives remain raw
executables, not signed application bundles with a Dock icon.

To regenerate from the original (ImageMagick 7):

```sh
magick assets/branding/logo.png -strip -resize 256x256 assets/branding/icon.png
magick assets/branding/logo.png -strip -define icon:auto-resize=256,128,64,48,32,16 assets/branding/icon.ico
magick assets/branding/logo.png -strip -resize 32x32 web/public/favicon.png
```
