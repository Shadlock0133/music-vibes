# Music Vibes - vibe with your music

\[WIP] (Windows-only for now) Translates currently playing audio into
vibrations, using connected [`buttplug`](https://buttplug.io/)-compatible
hardware

![gif](./mv.gif)

## Installing

0. You will require working Rust toolchain. You can install it is by using [rustup](https://rustup.rs/).

1. Clone the repo with `git clone https://github.com/Shadlock0133/music-vibes.git`
or download it clicking in top-right corner `Code` > `Download ZIP`
  
2. Install with `cargo install --path .`

3. (optional) You can also build without installing using `cargo build --release`,
which creates executable at `target/release/music-vibes{.exe}`

## Caveats

Created mostly to play around with qdot's excellent `buttplug` and my own
`audio-capture` crate.

Current implementation of cutoff filter is "sharp", that is, it will jump from
zero to above set `min` value, with no smoothing, so be careful with that.
