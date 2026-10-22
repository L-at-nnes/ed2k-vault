//! Path helpers: sequential file opening and Windows extended-length paths.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Opens a file for reading, hinting the OS that it will be read sequentially.
pub fn open_sequential(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;
        options.custom_flags(FILE_FLAG_SEQUENTIAL_SCAN);
    }
    options.open(path)
}

/// Absolute form of `path` that also works past the 260 character limit on
/// Windows (`\\?\` and `\\?\UNC\` prefixes). Other platforms get a plain
/// canonical path.
pub fn extended(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

/// Human friendly form of an extended path (drops the `\\?\` prefix).
pub fn display(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_strips_the_extended_prefix() {
        assert_eq!(display(Path::new(r"\\?\C:\data\a.bin")), r"C:\data\a.bin");
    }

    #[test]
    fn display_restores_unc_paths() {
        assert_eq!(
            display(Path::new(r"\\?\UNC\nas\media\a.bin")),
            r"\\nas\media\a.bin"
        );
    }

    #[test]
    fn display_leaves_plain_paths_alone() {
        assert_eq!(display(Path::new("/srv/data/a.bin")), "/srv/data/a.bin");
    }
}
