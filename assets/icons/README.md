# Workspace icons

Six static Kubai Icons from `kubai087/kubai-icons` at commit
`c583030f168294d0e1926e1e7a4fbcf4ee0676ae`, rendered from the MIT-licensed
`icons/` definitions with `packages/core/src/render.js` (also MIT).
The accompanying `LICENSE` is upstream `LICENSE-ICONS`.

Selection: monitor (Desktop), terminal, folder (Files), clipboard,
shield (Session / host permissions), info (context help).

Export settings: `renderSVG(def, { accent: '#7193B5', size: 24, hover: false })`;
default square canvas and padding preserve the layered outlines and shadows.
The blue-gray palette is fixed asset data, not a replacement for semantic UI
theme tokens. Controls, focus, status, permissions and errors continue to use
their existing tokens and visible labels in both appearances.

Only these standalone SVGs ship. No animation engine, skill rules, website,
React/Vue package or network icon loader is included. Web instances namespace
SVG definition IDs; NSH parses each SVG as an isolated native drawing. Icons
are decorative and never replace accessible action names or the owner's logo.
The Web distribution includes `kubai-icons-LICENSE.txt`; desktop archives
include `assets/icons/LICENSE`. Native desktop and Android also embed and show
the full notice in the optional Icon licenses disclosure.
