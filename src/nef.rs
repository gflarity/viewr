// Adapted from LightCraft (https://github.com/storytold/lightcraft),
// Copyright (c) 2026 ArtCraft Team and the LightCraft contributors.
// LightCraft is licensed MIT OR Apache-2.0; viewr uses the Apache-2.0 option.
// Upstream: crates/raw/src/preview.rs, trimmed to the TIFF-based (NEF)
// embedded-JPEG path. Changes from upstream: see ATTRIBUTION.md.

//! Embedded preview extraction: the largest baseline/progressive JPEG stored in a TIFF-based raw
//! (IFD strips with JPEG compression, `JPEGInterchangeFormat` pointers in any IFD, Nikon's
//! maker-note preview IFD), or — as a last resort — the largest complete colour JPEG found by
//! scanning the file.

use crate::tiff::image::chunk_bytes;
use crate::tiff::tags as t;
use crate::tiff::{Ifd, Tiff, makernote};

/// Whether `b` looks like a displayable (DCT) JPEG: SOI, and the first SOF marker is not lossless.
fn is_dct_jpeg(b: &[u8]) -> bool {
    if b.len() < 4 || b[0] != 0xff || b[1] != 0xd8 {
        return false;
    }
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xff {
            return false;
        }
        let m = b[i + 1];
        if m == 0xff {
            i += 1;
            continue;
        }
        match m {
            0xc0..=0xc2 => return true,
            0xc3 | 0xc5..=0xc7 | 0xcb | 0xcd..=0xcf => return false,
            0xda | 0xd9 => return false,
            _ => {}
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        i += 2 + len;
    }
    false
}

fn candidates<'a>(data: &'a [u8], ifd: &Ifd, base: u64, out: &mut Vec<&'a [u8]>) {
    // whole JPEG files stored as an undefined-type tag value (e.g. Panasonic `JpgFromRaw` 0x002e)
    for e in &ifd.entries {
        if matches!(e.value, crate::tiff::Value::Undefined(_))
            && e.count() > 1024
            && let Some(s) = data.get(e.offset as usize..(e.offset.saturating_add(e.count() as u64) as usize).min(data.len()))
            && s.starts_with(&[0xff, 0xd8])
        {
            out.push(s);
        }
    }
    if let (Some(off), Some(len)) = (ifd.u64(t::JPEG_INTERCHANGE_FORMAT), ifd.u64(t::JPEG_INTERCHANGE_FORMAT_LENGTH)) {
        let off = off.saturating_add(base);
        if let Some(s) = data.get(off as usize..(off.saturating_add(len) as usize).min(data.len())) {
            out.push(s);
        }
    }
    if matches!(ifd.u16(t::COMPRESSION), Some(6) | Some(7) | Some(34892))
        && let Ok(info) = ifd.image()
    {
        let chunks = info.chunks(data.len() as u64);
        if chunks.len() == 1
            && let Some(s) = chunk_bytes(data, &chunks[0])
        {
            out.push(s);
        }
    }
}

/// The largest embedded preview JPEG, if any. Decodes with any baseline-JPEG decoder.
pub fn embedded_preview(bytes: &[u8]) -> Option<Vec<u8>> {
    let tiff = Tiff::parse(bytes).ok()?;
    let mut found: Vec<&[u8]> = Vec::new();
    for ifd in tiff.all_ifds() {
        candidates(bytes, ifd, 0, &mut found);
    }
    // maker-note preview IFDs (e.g. Nikon PreviewIFD 0x0011 holds JPEGInterchangeFormat relative to the note base)
    if let Some(exif) = tiff.exif()
        && let Some(e) = exif.get(t::MAKER_NOTE)
    {
        let make = tiff.find(t::MAKE).and_then(|e| e.value.as_str()).unwrap_or_default();
        if let Some(mn) = makernote::parse_makernote(bytes, e.offset, e.count() as u64, tiff.order, &make) {
            candidates(bytes, &mn.ifd, mn.base, &mut found);
            if let Some(off) = mn.ifd.u64(0x0011)
                && let Ok((pifd, _)) = crate::tiff::parse_ifd_at(bytes, mn.base + off, mn.order, mn.base, false, &Default::default())
            {
                candidates(bytes, &pifd, mn.base, &mut found);
            }
        }
    }
    let best = found.into_iter().filter(|s| is_dct_jpeg(s)).max_by_key(|s| s.len()).map(|s| trim_eoi(s).to_vec());
    // a TIFF raw whose preview no tag points to: last-resort scan for a stored JPEG
    best.or_else(|| scan_for_jpeg(bytes))
}

/// The end of the DCT JPEG whose SOI is at `soi` (exclusive index after its EOI) and its component count,
/// walking the marker segments and the entropy-coded data. `None` when it is not a complete, displayable
/// (baseline or progressive) JPEG, so a stray `D8 FF` in other data is rejected. `*reach` is left at
/// the furthest byte looked at (what the walk cost; see [`scan_for_jpeg`]).
fn jpeg_extent(b: &[u8], soi: usize, reach: &mut usize) -> Option<(usize, u8)> {
    let mut i = soi + 2;
    let r = walk_jpeg(b, &mut i);
    *reach = i;
    r
}

fn walk_jpeg(b: &[u8], i: &mut usize) -> Option<(usize, u8)> {
    let mut components = None;
    for _ in 0..4096 {
        if *b.get(*i)? != 0xff {
            return None;
        }
        while *b.get(*i)? == 0xff {
            *i += 1;
        }
        let m = *b.get(*i)?;
        *i += 1;
        match m {
            0xd9 => return components.map(|c| (*i, c)),
            0x01 | 0xd0..=0xd8 => continue,
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([*b.get(*i)?, *b.get(*i + 1)?]));
        if len < 2 {
            return None;
        }
        match m {
            0xc0..=0xc2 => {
                // precision, height, width, component count
                let h = u16::from_be_bytes([*b.get(*i + 3)?, *b.get(*i + 4)?]);
                let w = u16::from_be_bytes([*b.get(*i + 5)?, *b.get(*i + 6)?]);
                if h == 0 || w == 0 || components.is_some() {
                    return None;
                }
                components = Some(*b.get(*i + 7)?);
            }
            0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => return None, // lossless, arithmetic, hierarchical
            _ => {}
        }
        *i += len;
        if m == 0xda {
            // entropy-coded data up to the next marker that is not a stuffed 0xff or a restart
            loop {
                *i += b.get(*i..)?.iter().position(|&x| x == 0xff)?;
                match *b.get(*i + 1)? {
                    0x00 | 0xd0..=0xd7 => *i += 2,
                    0xff => *i += 1,
                    _ => break,
                }
            }
        }
    }
    None
}

/// Last resort for files whose preview no tag points to: the largest complete colour DCT JPEG stored as a
/// plain byte run in the file. A one-component JPEG is skipped (a mosaic stored as one must not win).
///
/// Evidence: in the Minolta and Epson files the first byte of the stored preview is not `ff` (it is `00`, `02` or
/// `ee`) while the rest is a regular JPEG, so a run is found by its `d8 ff` and starts one byte earlier,
/// with that byte restored to `ff`.
///
/// A crafted file can hold millions of JPEG-like starts whose walks each run to its end: the walks
/// share a budget of eight times the file's size, so the scan stays linear (a real file's candidates
/// fail within a few bytes, or are the preview itself).
fn scan_for_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    let mut best: Option<(usize, usize)> = None;
    let mut at = 1;
    let mut budget = data.len().saturating_mul(8).max(1 << 20);
    while let Some(off) = data.get(at..).and_then(|s| s.iter().position(|&b| b == 0xd8)) {
        let d8 = at + off;
        at = d8 + 1;
        if data.get(d8 + 1) != Some(&0xff) || !matches!(data.get(d8 + 2), Some(0xc0..=0xc4 | 0xdb | 0xe0..=0xef | 0xfe)) {
            continue;
        }
        let start = d8 - 1; // `at` starts at 1, so d8 >= 1
        let mut reach = start;
        let found = jpeg_extent(data, start, &mut reach);
        budget = budget.saturating_sub(reach.saturating_sub(start));
        if budget == 0 {
            break;
        }
        if let Some((end, 3)) = found {
            if best.is_none_or(|(s, e)| end - start > e - s) {
                best = Some((start, end));
            }
            at = end;
        }
    }
    let (start, end) = best?;
    let mut jpeg = data.get(start..end)?.to_vec();
    if let Some(first) = jpeg.first_mut() {
        *first = 0xff;
    }
    Some(jpeg)
}

/// Trim trailing garbage after the last EOI when a stored length over-reports.
fn trim_eoi(s: &[u8]) -> &[u8] {
    let end = s.windows(2).rposition(|w| w == [0xff, 0xd9]).map(|p| p + 2).unwrap_or(s.len());
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiff::{IfdBuilder, ImageData, TiffWriter, Value};

    fn fake_jpeg(n: usize) -> Vec<u8> {
        let mut j = vec![0xff, 0xd8, 0xff, 0xc0, 0x00, 0x0b, 8, 0, 1, 0, 1, 1, 1, 0x11, 0];
        j.extend(std::iter::repeat_n(0x55u8, n));
        j.extend_from_slice(&[0xff, 0xd9]);
        j
    }

    /// A minimal lossless (SOF3) JPEG: structurally parallel to `fake_jpeg` but never a candidate.
    fn fake_lossless_jpeg() -> Vec<u8> {
        let mut j = vec![0xff, 0xd8, 0xff, 0xc3, 0x00, 0x0b, 8, 0, 1, 0, 1, 1, 1, 0x11, 0];
        j.extend_from_slice(&[0x55u8; 64]);
        j.extend_from_slice(&[0xff, 0xd9]);
        j
    }

    #[test]
    fn picks_largest_dct_jpeg() {
        let small = fake_jpeg(10);
        let big = fake_jpeg(500);
        let lossless = fake_lossless_jpeg();
        let mut ifd0 = IfdBuilder::new();
        ifd0.set(t::COMPRESSION, Value::Short(vec![6]));
        ifd0.set(t::IMAGE_WIDTH, Value::Long(vec![1]));
        ifd0.set(t::IMAGE_LENGTH, Value::Long(vec![1]));
        ifd0.set_image(ImageData::Strips { rows_per_strip: 1, strips: vec![small.clone()] });
        let mut raw = IfdBuilder::new();
        raw.set(t::COMPRESSION, Value::Short(vec![7]));
        raw.set(t::IMAGE_WIDTH, Value::Long(vec![8]));
        raw.set(t::IMAGE_LENGTH, Value::Long(vec![8]));
        raw.set_image(ImageData::Strips { rows_per_strip: 8, strips: vec![lossless] });
        ifd0.add_sub_ifd(raw);
        let mut sub = IfdBuilder::new();
        sub.set(t::COMPRESSION, Value::Short(vec![7]));
        sub.set(t::IMAGE_WIDTH, Value::Long(vec![2]));
        sub.set(t::IMAGE_LENGTH, Value::Long(vec![2]));
        sub.set(t::NEW_SUBFILE_TYPE, Value::Long(vec![1]));
        sub.set_image(ImageData::Strips { rows_per_strip: 2, strips: vec![big.clone()] });
        ifd0.add_sub_ifd(sub);
        let bytes = TiffWriter::default().write(&[ifd0]).unwrap();
        assert_eq!(embedded_preview(&bytes).unwrap(), big);
        assert!(embedded_preview(b"nope").is_none());
    }

    /// A little-endian TIFF whose IFD0 points at the preview with JPEGInterchangeFormat(513/514),
    /// the way classic NEFs do.
    fn tiff_with_interchange_preview(jpeg: &[u8]) -> Vec<u8> {
        let mut b = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
        let jpeg_at = 8u32 + 2 + 2 * 12 + 4;
        b.extend_from_slice(&[2, 0]);
        for (tag, value) in [(t::JPEG_INTERCHANGE_FORMAT, jpeg_at), (t::JPEG_INTERCHANGE_FORMAT_LENGTH, jpeg.len() as u32)] {
            b.extend_from_slice(&tag.to_le_bytes());
            b.extend_from_slice(&4u16.to_le_bytes()); // LONG
            b.extend_from_slice(&1u32.to_le_bytes());
            b.extend_from_slice(&value.to_le_bytes());
        }
        b.extend_from_slice(&[0, 0, 0, 0]); // no next IFD
        debug_assert_eq!(b.len() as u32, jpeg_at);
        b.extend_from_slice(jpeg);
        b
    }

    #[test]
    fn follows_jpeg_interchange_format_pointers() {
        let jpeg = fake_jpeg(300);
        let bytes = tiff_with_interchange_preview(&jpeg);
        assert_eq!(embedded_preview(&bytes).unwrap(), jpeg);
        // a truncated length is clipped to the file rather than rejected
        let mut short = bytes.clone();
        let len_at = 8 + 2 + 12 + 8;
        short[len_at..len_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(embedded_preview(&short).unwrap(), jpeg);
    }

    /// A structurally complete DCT JPEG (no real image data): `nc` components, `n` entropy bytes.
    fn scan_jpeg(nc: u8, n: usize) -> Vec<u8> {
        let mut j = vec![0xff, 0xd8, 0xff, 0xdb, 0x00, 0x43, 0x00];
        j.extend(std::iter::repeat_n(8u8, 64));
        j.extend_from_slice(&[0xff, 0xc0, 0x00, 8 + 3 * nc, 8, 0, 16, 0, 24, nc]);
        for c in 0..nc {
            j.extend_from_slice(&[c + 1, 0x11, 0]);
        }
        j.extend_from_slice(&[0xff, 0xda, 0x00, 6 + 2 * nc, nc]);
        for c in 0..nc {
            j.extend_from_slice(&[c + 1, 0]);
        }
        j.extend_from_slice(&[0, 63, 0]);
        j.extend(std::iter::repeat_n(0x55u8, n));
        j.extend_from_slice(&[0xff, 0x00, 0x12, 0xff, 0xd0, 0x34, 0xff, 0xd9]);
        j
    }

    /// A crafted container full of JPEG-like starts (each walk running on through the ones after it)
    /// is scanned in linear time: the walks share a budget (without it this 40 MB file takes about a minute).
    #[test]
    fn a_scan_through_many_fake_jpegs_stays_linear() {
        let mut unit = vec![0u8, 0xd8, 0xff, 0xdb, 0x00, 0x02, 0xff, 0xda, 0x00, 0x02];
        unit.extend(std::iter::repeat_n(0x55u8, 190));
        let mut f = b"\0MRM\0\x01\0\0".to_vec();
        for _ in 0..200_000 {
            f.extend_from_slice(&unit);
        }
        let t = std::time::Instant::now();
        assert_eq!(scan_for_jpeg(&f), None);
        assert!(t.elapsed() < std::time::Duration::from_secs(10), "{:?}", t.elapsed());
    }

    /// Files whose preview no tag points to give up their largest colour JPEG when scanned.
    #[test]
    fn scanned_containers_give_the_largest_colour_jpeg() {
        let (small, big, mosaic) = (scan_jpeg(3, 100), scan_jpeg(3, 900), scan_jpeg(1, 5000));
        let heads: [&[u8]; 3] = [b"II\x1a\0\0\0HEAPCCDR", b"\0MRM\0\x01\0\0", b"FOVb\x02\0\x02\0"];
        for head in heads {
            let mut f = head.to_vec();
            for part in [&small, &mosaic, &big] {
                f.extend_from_slice(&[0xff, 0xd8, 0xff, 0x00, 0x00]); // a stray SOI-like run: not a JPEG
                f.extend_from_slice(part);
                f.extend_from_slice(&[0u8; 7]);
            }
            assert_eq!(scan_for_jpeg(&f).as_deref(), Some(&big[..]), "{head:?}");
        }
    }
}
