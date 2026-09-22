# Slix Player

A lightweight music player built with [Rust](https://www.rust-lang.org/) & [Slint](https://slint.dev/)

![Slix Player](./screenshot.png)

Currently only tested a little bit on Arch & OSX, but there are builds for windows to check out as well.

## Usage

Open it up and point to a folder with music files.  It'll recursively scan, analyse and add them.

## Features

Basically should be the same as any audio player you're used to, with play/pause etc..

But it does extract some info about the tracks & maintains your last playing state so you can resume where you left off.

* Analyses tracks and extracts metadata/cover art and builds a waveform.
* Cross platform UI with [Slint](https://slint.dev/)
* Track analysis with [`Symphonia`](https://github.com/pdeljanov/Symphonia)
* Track cache using [`fjall`](https://fjall-rs.github.io/)
* Playback using [`rodio`](https://github.com/RustAudio/rodio)
* Media Control Integration with [`souvlaki`](https://github.com/Sinono3/souvlaki)

## Keyboard Shortcuts

| Key | Action |
|---|---|
| <kbd>Space</kbd> | Play / pause |
| <kbd>←</kbd> / <kbd>→</kbd> | Seek back / forward 5 seconds |
| <kbd>Shift</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | Seek back / forward 30 seconds |
| <kbd>0</kbd>–<kbd>9</kbd> | Jump to 0%–90% of the track |
| <kbd>Home</kbd> | Restart the track |
| <kbd>Ctrl</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | Previous / next track |
| <kbd>↑</kbd> / <kbd>↓</kbd> | Volume up / down |
| <kbd>M</kbd> | Mute / unmute |
| <kbd>S</kbd> | Toggle shuffle |
| <kbd>L</kbd> | Scroll to the playing track |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> or <kbd>/</kbd> | Search |
| <kbd>Enter</kbd> (in search) | Play the first match |
| <kbd>Esc</kbd> (in search) | Clear and close search |
| <kbd>Ctrl</kbd>+<kbd>O</kbd> | Open a music folder |
| <kbd>Ctrl</kbd>+<kbd>Q</kbd> | Quit |

On macOS, use <kbd>Cmd</kbd> in place of <kbd>Ctrl</kbd>.

*Note: This is a bit of an alpha project and there are probably bugs.  Feel free to raise an issue or a PR!*

## Downloads

You can download the latest release for your platform from the [releases page](https://github.com/cetra3/slix-player/releases).



## Compiling

You will need to ensure that you have the appropriate prereqs for slint

I.e,

```
sudo apt install -y build-essential libx11-xcb1 libx11-dev libxcb1-dev libxkbcommon0 libinput10 libinput-dev libgbm1 libgbm-dev
```

Then run:

```
cargo build --release
```

## Installing on Gnome

Make sure you can compile it, then run from this dir:

```
cargo install --path .
```

Make sure the cargo bin is on your path.

Then you can create a desktop entry by running:

```
./install_desktop_entry.sh
```
