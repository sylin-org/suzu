//! Animated GIF89a — the recorder's output format.
//!
//! Frames arrive as truecolor RGBA views (decoded per the class
//! manifest); the maintained `gif` crate quantizes and LZW-encodes —
//! the bit-level dance is proven code, ours is only the timing and
//! the loop.

use anyhow::Result;

/// Write an animated, infinitely-looping GIF89a. `frames` are flat
/// RGBA buffers of `w * h * 4` bytes each; `delay_cs` is per-frame
/// delay in centiseconds.
pub fn write_gif_rgba(
    path: &std::path::Path,
    w: usize,
    h: usize,
    delay_cs: u16,
    frames: &[Vec<u8>],
) -> Result<()> {
    use gif::{Encoder, Frame, Repeat};
    use std::fs::File;

    let file = File::create(path)?;
    let mut encoder = Encoder::new(file, w as u16, h as u16, &[])?;
    encoder.set_repeat(Repeat::Infinite)?;

    for frame in frames {
        let mut indexed = frame.clone(); // from_rgba quantizes in place
        let mut f = Frame::from_rgba(w as u16, h as u16, &mut indexed);
        f.delay = delay_cs;
        encoder.write_frame(&f)?;
    }
    Ok(())
}
