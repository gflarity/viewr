//! Background decoding: the UI thread asks for indices, a worker thread decodes them
//! (JPEG/PNG directly, NEF via its embedded preview) and posts the pixels back.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

#[derive(Debug)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct Loaded {
    pub index: usize,
    pub result: Result<Decoded, String>,
}

pub struct Loader {
    req: Sender<Vec<usize>>,
    pub res: Receiver<Loaded>,
}

impl Loader {
    pub fn spawn(files: Vec<PathBuf>) -> Self {
        let (req_tx, req_rx) = channel::<Vec<usize>>();
        let (res_tx, res_rx) = channel::<Loaded>();
        thread::spawn(move || {
            while let Ok(first) = req_rx.recv() {
                // a held-down arrow key queues faster than we decode: work the latest
                // request only, so the image on screen is never stuck behind stale ones
                let mut latest = first;
                while let Ok(next) = req_rx.try_recv() {
                    latest = next;
                }
                for idx in latest {
                    if let Some(path) = files.get(idx) {
                        let result = decode_file(path);
                        if res_tx.send(Loaded { index: idx, result }).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        Self { req: req_tx, res: res_rx }
    }

    /// Ask for `indices` in order (current image first, then prefetch neighbours).
    pub fn request(&self, indices: Vec<usize>) {
        let _ = self.req.send(indices);
    }
}

fn decode_pixels(bytes: &[u8], orient_from: &[u8]) -> Result<Decoded, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let mut rgba = img.to_rgba8();
    if let Some(o) = crate::orient::orientation(orient_from).filter(|o| *o != 1) {
        rgba = crate::orient::apply(rgba, o);
    }
    Ok(Decoded { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() })
}

pub fn decode_file(path: &Path) -> Result<Decoded, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if crate::files::is_nef(path) {
        match crate::nef::embedded_preview(&bytes) {
            Some(preview) => decode_pixels(&preview, &preview).map_err(|e| format!("{}: preview: {e}", path.display())),
            None => Err(format!("{}: no JPEG preview found in NEF", path.display())),
        }
    } else {
        decode_pixels(&bytes, &bytes).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_errors() {
        let err = decode_file(Path::new("/no/such/dir/viewr-test-12345.jpg")).unwrap_err();
        assert!(err.contains("No such file"), "{err}");
    }

    #[test]
    fn non_image_errors() {
        let dir = std::env::temp_dir().join(format!("viewr-test-loader-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.jpg");
        std::fs::write(&path, b"definitely not a jpeg").unwrap();
        assert!(decode_file(&path).is_err());
        let nef = dir.join("x.nef");
        std::fs::write(&nef, b"definitely not a nef").unwrap();
        let err = decode_file(&nef).unwrap_err();
        assert!(err.contains("no JPEG preview"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
