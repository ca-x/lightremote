# LightRemote workstation

1. **Theme.** Calm utility workstation. Clear operational state and a generous working surface, with no marketing content.
2. **Palette.** Ink `#111918` canvas; raised ink `#192321` main surface; chalk `#edf2eb` text; sage `#a4b6ae` secondary; teal `#88dcc8` actions; amber `#efbc79` waiting; red `#f49b92` errors.
3. **Type.** System humanist sans (`Segoe UI`, system-ui) for native familiarity, Georgia for the small product wordmark, system monospace for identifiers and terminal. Scale 12/14/16/24/32px, line height 1.5.
4. **Components.** Buttons and inputs use 6px radius, 42px minimum height, visible 2px teal focus. Navigation uses a quiet active surface and teal glyph. Errors are persistent text, status has both words and color.
5. **Layout.** 224px sidebar, fluid workspace. Spacing 4/8/12/16/24/32/48px. Distinct sections use rules and spacing, not repeated cards.
6. **Depth.** Flat opaque luminance steps; no decorative gradients, glass, or ornamental shadows. Remote content is the visual anchor when connected.
7. **Guardrails.** No mock desktop. No automatic clipboard read. No secret in logs, storage or query. No unattended input authorization. No success message before host response. Use local fonts and assets.
8. **Responsive.** Below 760px navigation becomes a compact top row. Toolbars wrap. Tables scroll within their region. Desktop retains its intrinsic aspect ratio. Touch controls stay at least 42px.
9. **Implementation guide.** Plain CSS semantic tokens. Build session status in muted sage with teal for connected state; build desktop stage in ink with an explicit unavailable message; build files as a dense accessible table with visible names and actions. Motion limited to 120ms color/opacity changes and disabled under reduced motion.
