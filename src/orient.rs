//! EXIF orientation: the stored pixels' row 0 is not always the scene's top row
//! (phone photos especially), so read tag 274 from JPEG APP1 Exif / PNG eXIf with the vendored
//! TIFF reader and rotate the decoded pixels to match.

use crate::tiff::{Tiff, tags};

/// EXIF orientation (1–8) of the image in `file_bytes`, if it declares one.
pub fn orientation(file_bytes: &[u8]) -> Option<u16> {
    let tiff_bytes = jpeg_exif_tiff(file_bytes).or_else(|| png_exif_tiff(file_bytes))?;
    let tiff = Tiff::parse(tiff_bytes).ok()?;
    let o = tiff.ifds.first()?.u16(tags::ORIENTATION)?;
    (1..=8).contains(&o).then_some(o)
}

/// The TIFF payload of a JPEG's APP1 Exif segment, if present.
fn jpeg_exif_tiff(b: &[u8]) -> Option<&[u8]> {
    if b.len() < 4 || b[0] != 0xff || b[1] != 0xd8 {
        return None;
    }
    let mut i = 2;
    loop {
        if b.get(i) != Some(&0xff) {
            return None;
        }
        while b.get(i) == Some(&0xff) {
            i += 1;
        }
        let m = *b.get(i)?;
        i += 1;
        match m {
            0xd9 | 0xda => return None,     // EOI / SOS: no EXIF beyond this point
            0x01 | 0xd0..=0xd8 => continue, // standalone markers carry no length
            _ => {}
        }
        let len = u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize;
        if len < 2 {
            return None;
        }
        let body = b.get(i + 2..i + len)?;
        if m == 0xe1 && body.starts_with(b"Exif\0\0") {
            return Some(&body[6..]);
        }
        i += len;
    }
}

/// The TIFF payload of a PNG's eXIf chunk, if present.
fn png_exif_tiff(b: &[u8]) -> Option<&[u8]> {
    if b.len() < 8 || b[..8] != *b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let mut i = 8;
    while let Some(head) = b.get(i..i + 8) {
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let data = b.get(i + 8..i + 8 + len)?;
        if head[4..8] == *b"eXIf" {
            return Some(data);
        }
        if head[4..8] == *b"IEND" {
            return None;
        }
        i = i.checked_add(8)?.checked_add(len)?.checked_add(4)?;
    }
    None
}

/// Rotate/flip stored pixels into display orientation (EXIF 1–8; anything else is returned as-is).
pub fn apply(mut img: image::RgbaImage, o: u16) -> image::RgbaImage {
    use image::imageops::{flip_horizontal_in_place, flip_vertical_in_place, rotate90, rotate180_in_place, rotate270};
    match o {
        1 => img,
        2 => {
            flip_horizontal_in_place(&mut img);
            img
        }
        3 => {
            rotate180_in_place(&mut img);
            img
        }
        4 => {
            flip_vertical_in_place(&mut img);
            img
        }
        5 => {
            let mut r = rotate90(&img);
            flip_horizontal_in_place(&mut r);
            r
        }
        6 => rotate90(&img),
        7 => {
            let mut r = rotate90(&img);
            flip_vertical_in_place(&mut r);
            r
        }
        8 => rotate270(&img),
        _ => img,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiff::{IfdBuilder, TiffWriter, Value};

    /// A 3×2 image with distinct pixels, row-major: [0 1 2; 3 4 5].
    fn distinct() -> image::RgbaImage {
        image::RgbaImage::from_fn(3, 2, |x, y| image::Rgba([(y * 3 + x) as u8, 0, 0, 255]))
    }

    fn reds(img: &image::RgbaImage) -> Vec<u8> {
        img.pixels().map(|p| p[0]).collect()
    }

    #[test]
    fn all_orientations_map_pixels() {
        // each (orientation, width, height, expected red channel row-major), worked out by hand
        // from the EXIF orientation spec
        let cases: [(u16, u32, u32, &[u8]); 8] = [
            (1, 3, 2, &[0, 1, 2, 3, 4, 5]),
            (2, 3, 2, &[2, 1, 0, 5, 4, 3]),
            (3, 3, 2, &[5, 4, 3, 2, 1, 0]),
            (4, 3, 2, &[3, 4, 5, 0, 1, 2]),
            (5, 2, 3, &[0, 3, 1, 4, 2, 5]),
            (6, 2, 3, &[3, 0, 4, 1, 5, 2]),
            (7, 2, 3, &[5, 2, 4, 1, 3, 0]),
            (8, 2, 3, &[2, 5, 1, 4, 0, 3]),
        ];
        for (o, w, h, expected) in cases {
            let out = apply(distinct(), o);
            assert_eq!((out.width(), out.height()), (w, h), "orientation {o}");
            assert_eq!(reds(&out), expected, "orientation {o}");
        }
        // unknown values are a no-op
        assert_eq!(reds(&apply(distinct(), 0)), reds(&distinct()));
        assert_eq!(reds(&apply(distinct(), 9)), reds(&distinct()));
    }

    fn tiff_with_orientation(o: u16) -> Vec<u8> {
        let mut ifd0 = IfdBuilder::new();
        ifd0.set(tags::ORIENTATION, Value::Short(vec![o]));
        TiffWriter::default().write(&[ifd0]).unwrap()
    }

    fn jpeg_with_app1(payloads: &[&[u8]], trailing: &[u8]) -> Vec<u8> {
        let mut b = vec![0xff, 0xd8];
        for p in payloads {
            b.push(0xff);
            b.push(0xe1);
            let len = p.len() + 2;
            b.extend_from_slice(&(len as u16).to_be_bytes());
            b.extend_from_slice(p);
        }
        b.extend_from_slice(trailing);
        b
    }

    #[test]
    fn reads_jpeg_exif_orientation() {
        let tiff = tiff_with_orientation(6);
        let mut exif = b"Exif\0\0".to_vec();
        exif.extend_from_slice(&tiff);
        let jpeg = jpeg_with_app1(&[&exif], &[0xff, 0xd9]);
        assert_eq!(orientation(&jpeg), Some(6));
        // the Exif segment need not be the first APP1
        let jpeg = jpeg_with_app1(&[b"http://ns.adobe.com/xap/1.0/\0..." as &[u8], &exif], &[0xff, 0xd9]);
        assert_eq!(orientation(&jpeg), Some(6));
        assert_eq!(orientation(b"not a jpeg"), None);
        assert_eq!(orientation(&[0xff, 0xd8, 0xff, 0xd9]), None);
        // garbage orientation values are ignored
        let tiff = tiff_with_orientation(42);
        let mut exif = b"Exif\0\0".to_vec();
        exif.extend_from_slice(&tiff);
        assert_eq!(orientation(&jpeg_with_app1(&[&exif], &[])), None);
    }

    fn png_with_chunks(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        for (kind, data) in chunks {
            b.extend_from_slice(&(data.len() as u32).to_be_bytes());
            b.extend_from_slice(*kind);
            b.extend_from_slice(data);
            b.extend_from_slice(&[0, 0, 0, 0]); // CRC is not checked
        }
        b
    }

    #[test]
    fn reads_png_exif_orientation() {
        let tiff = tiff_with_orientation(3);
        let png = png_with_chunks(&[(b"IHDR", &[0u8; 13]), (b"eXIf", &tiff), (b"IEND", &[])]);
        assert_eq!(orientation(&png), Some(3));
        let png = png_with_chunks(&[(b"IHDR", &[0u8; 13]), (b"IEND", &[])]);
        assert_eq!(orientation(&png), None);
        assert_eq!(orientation(b"not a png at all...."), None);
    }
}
