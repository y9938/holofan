# HoloFan

Editor and encoder for the rotating LED POV display format verified with the supplied controller. It imports still images and animations, arranges them in a playlist, previews and transforms frames, then writes a controller-specific `.bin` file for removable USB or SD storage.

The supported profile uses 112 radial LEDs and 450 angular lines. Each 114,688-byte frame contains six packed color bitplanes and controller padding. Ordinary image files cannot be copied to the display as-is.

USB HID is separate: HoloFan can query controller status and read a memory dump for supported devices (VID:PID `0483:5750`). It does not send image or playlist data over USB.

## Features

- Import JPEG, PNG, BMP, GIF, and WebP images
- Arrange items in a playlist and set duration
- Set each item's controller speed (1–255; new items default to 50)
- Adjust framing, zoom, rotation, and mirroring
- Preview the encoded circular frame
- Save `.bin` files or write them to removable storage
- Run on Windows and Linux

The encoder targets the verified 112 × 450 controller format. Compatibility with other fan models has not been established.

## Build

Install the Rust toolchain, then build and run:

```bash
git clone https://github.com/y9938/holofan.git
cd holofan
cargo run --release
```

JPEG decoding uses bundled IJG 8c source, so the application does not depend on a system JPEG library. This software is based in part on the work of the Independent JPEG Group.

## Command line

```bash
# Inspect a controller file
cargo run --release -- info -f hologram.bin

# Create a file from an image, animation, or directory of images
cargo run --release -- create -i animation.gif -o hologram.bin

# Match a one-frame still-image export with speed 50
cargo run --release -- create -i still.jpg -o still.bin --speed 50 --repeat 1

# Extract frames as PNG previews
cargo run --release -- unpack -i hologram.bin -o frames/
```
