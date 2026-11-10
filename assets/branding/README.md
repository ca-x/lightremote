# roc-remote branding

The eagle logo was supplied and approved by the project owner on 2026-11-10.
`logo.png` preserves the uploaded original. `icon.png`, `icon.ico` and the Web
favicon are size-only derivatives, generated with ImageMagick; no reference
project artwork was copied.

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
