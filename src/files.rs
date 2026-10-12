//! The images to view: supported files directly inside one directory, sorted by name.

use std::path::{Path, PathBuf};

/// Extensions viewr opens, matched case-insensitively.
pub fn is_supported(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => matches!(ext.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "nef"),
        None => false,
    }
}

pub fn is_nef(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some(ext) if ext.eq_ignore_ascii_case("nef"))
}

/// Supported files directly inside `dir`, sorted by file name.
pub fn collect(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && is_supported(&entry.path()) {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_extensions() {
        for name in ["a.jpg", "a.JPG", "a.jpeg", "a.JPEG", "a.png", "a.PNG", "a.nef", "a.NEF"] {
            assert!(is_supported(Path::new(name)), "{name}");
        }
        for name in ["a.gif", "a.tif", "a.nef.bak", "jpg", "a", ".jpg"] {
            assert!(!is_supported(Path::new(name)), "{name}");
        }
        assert!(is_nef(Path::new("a.NEF")));
        assert!(!is_nef(Path::new("a.jpg")));
    }

    #[test]
    fn collects_sorted_supported_files_only() {
        let dir = std::env::temp_dir().join(format!("viewr-test-collect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["b.png", "a.JPG", "skip.txt", "c.nef"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.join("d.jpg")).unwrap(); // a directory named *.jpg is not an image
        let found = collect(&dir).unwrap();
        let names: Vec<_> = found.iter().map(|p| p.file_name().unwrap().to_str().unwrap().to_string()).collect();
        assert_eq!(names, vec!["a.JPG", "b.png", "c.nef"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_missing_dir_errors() {
        assert!(collect(Path::new("/no/such/dir/viewr-test-12345")).is_err());
    }
}
