# Music Vibes - vibe with your music

!!! VERY WIP !!!

Turns audio into vibrations, using connected
[`buttplug`](https://buttplug.io/)-compatible hardware

## Caveats

Created mostly to play around with qdot's excellent `buttplug` and my own
`audio-capture` crate.

Right now it's just a cli program with two mode:
- play - takes audio file and plays it (slightly broken, desyncs)
- listen - captures system audio (windows only, because `audio-capture`
doesn't implement anything else)

Currently, it can only handle one device, and will panic if there zero, two
or more devices connected. Probably want to add way to select devices,
together with some options (speed multiplier, upper limit).
