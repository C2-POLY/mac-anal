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

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
