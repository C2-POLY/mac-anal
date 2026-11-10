# mac-anal

A macOS app, in Rust and egui, that analyses the Mac.

## Build from source

You need macOS and Rust 1.95 or later, from rustup (`brew install rustup`; Homebrew's `rust` is too old).

```bash
cd app
cargo run                                  # open the app
cargo test                                 # unit tests
cargo run -- --screenshot /tmp/shot.png    # draw the window on sample data, save it and quit
```

`MAC_ANAL_APPEARANCE=light` or `dark` forces a theme, for a screenshot; without it the app follows the system.

## Use it from Claude (MCP)

`mac-anal mcp` is an MCP server on stdio. It reads the measures, the analysis and the journal, and runs only the reversible actions of the catalog; irreversible ones stay in the app. Every action it runs is journaled with the `MCP` origin and can be undone.

Claude Code, with the binary built by `cargo build --release`:

```bash
claude mcp add mac-anal -- /path/to/mac-anal/app/target/release/mac-anal mcp
```

Claude Desktop, in `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{ "mcpServers": { "mac-anal": { "command": "/path/to/mac-anal/app/target/release/mac-anal", "args": ["mcp"] } } }
```

## Design

The colours, fonts, logotype and icon follow the [design document](https://claude.ai/artifact/LrLhk84VLHVZVRL5dpCriD); its tokens live in `app/src/theme.rs`, the logotype and safety icons in `app/src/brand.rs`.

The window uses those tokens the way the product page does (`site/style.css`): a cream background (ink in the dark), a yellow sidebar, cards with a heavy edge and a hard shadow, titles in Bricolage Grotesque, pill buttons, and measures drawn in ink, coral and yellow. The shared pieces (`card`, `primary_button`, `eyebrow`, `field`) are in `app/src/theme.rs`; a screen calls them instead of drawing its own frame.

- Bricolage Grotesque, the brand font, ships in `app/assets/` under the SIL Open Font License (`OFL.txt`). The body text uses SF, read from `/System/Library/Fonts` at launch, and falls back to egui's font without it.
- `app/assets/` holds the icon sources from the document (`broom.svg`, `icon-foreground.svg`, `icon-legacy.svg`, `icon-small.svg`). The Dock icon is `icon.png`, drawn from `icon-legacy.svg`; after changing the SVG, draw it again:

```bash
cargo test -- --ignored icon_png_is_current
```

- `app/assets/menubar/` holds the glyphs of the menu bar widgets, one SVG each and the PNG drawn from it. They are template images of one size, so macOS tints them and they line up with the value beside them. After changing an SVG, draw the PNGs again:

```bash
cargo test -- --ignored menubar_glyphs_are_current
```

## Product page

`site/` is the product page, plain HTML, CSS and JS (GSAP and Lenis from a CDN), published to GitHub Pages by `.github/workflows/pages.yml` on every push to `main` that touches it. To look at it locally:

```bash
python3 -m http.server 4173 --directory site
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
