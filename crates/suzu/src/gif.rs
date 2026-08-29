//! Animated GIF89a — the recorder's output format.
//!
//! The face is a three-color instrument (dark, yellow strip, cyan
//! field), so the palette is four entries and the indices are one
//! byte-snack per pixel. The LZW encoder here is the GIF variant:
//! variable code width, clear code 1<<min, end code after, LSB-first
//! bit packing into <=255-byte sub-blocks. ~80 lines, no dependencies.


/// Write an animated GIF89a via the maintained `gif` crate — the LZW
/// bit-level dance is someone else's proven code; ours is only the
/// palette and the frames.
pub fn write_gif(
    path: &std::path::Path,
    w: usize,
    h: usize,
    delay_cs: u16,
    palette: &[[u8; 3]],
    frames: &[Vec<u8>],
) -> anyhow::Result<()> {
    use gif::Frame;
    use std::fs::File;

    let mut colors = Vec::with_capacity(palette.len() * 3);
    for c in palette {
        colors.extend_from_slice(c);
    }
    while colors.len() < 3 * 4 {
        colors.push(0); // the palette pads to 4 entries
    }

    let file = File::create(path)?;
    let mut encoder =
        gif::Encoder::new(file, w as u16, h as u16, &colors[..])?;
    encoder.set_repeat(gif::Repeat::Infinite)?;

    for frame in frames {
        let mut f = Frame {
            width: w as u16,
            height: h as u16,
            buffer: std::borrow::Cow::Borrowed(frame),
            delay: delay_cs,
            ..Default::default()
        };
        encoder.write_frame(&mut f)?;
    }
    Ok(())
}
