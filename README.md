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

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
