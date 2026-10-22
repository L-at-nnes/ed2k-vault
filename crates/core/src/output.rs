//! Streaming output writers: every entry is written as soon as it is hashed.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::str::FromStr;

use crate::error::{Error, Result};
use crate::hasher::FileDigest;
use crate::link::{self, LinkOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// One link per line.
    Links,
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "links" | "txt" => Ok(Format::Links),
            other => Err(format!("unknown format `{other}`")),
        }
    }
}

/// A hashed file as seen by the writers.
pub struct Entry<'a> {
    pub name: &'a str,
    pub digest: &'a FileDigest,
}

pub trait EntryWriter: Send {
    fn write_entry(&mut self, entry: &Entry) -> io::Result<()>;
    /// Writes any footer and flushes. Must be called once, even after a
    /// cancellation, so the file stays valid.
    fn finish(self: Box<Self>) -> io::Result<()>;
}

/// Where the output goes.
pub enum Target<'a> {
    File(&'a Path),
    Stdout,
}

const BUFFER_SIZE: usize = 1 << 20;

/// Opens `target` and returns the writer for `format`.
pub fn create(
    format: Format,
    target: Target,
    options: LinkOptions,
) -> Result<Box<dyn EntryWriter>> {
    let out: Box<dyn Write + Send> = match target {
        Target::File(path) => {
            let file = File::create(path).map_err(|e| Error::io(path, e))?;
            Box::new(BufWriter::with_capacity(BUFFER_SIZE, file))
        }
        Target::Stdout => Box::new(BufWriter::new(io::stdout())),
    };
    Ok(match format {
        Format::Links => Box::new(TextWriter {
            out,
            options,
            style: Style::Links,
        }),
    })
}

#[derive(Clone, Copy)]
enum Style {
    Links,
}

struct TextWriter {
    out: Box<dyn Write + Send>,
    options: LinkOptions,
    style: Style,
}

impl EntryWriter for TextWriter {
    fn write_entry(&mut self, entry: &Entry) -> io::Result<()> {
        let link = link::build(entry.name, entry.digest, self.options);
        match self.style {
            Style::Links => writeln!(self.out, "{link}"),
        }
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn digest() -> FileDigest {
        FileDigest {
            size: 42,
            hash: [1; 16],
            part_hashes: vec![[1; 16]],
            aich: Some([2; 20]),
        }
    }

    pub(super) fn render(format: Format, names: &[&str], options: LinkOptions) -> Vec<u8> {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = create(format, Target::File(file.path()), options).unwrap();
        let digest = digest();
        for name in names {
            writer.write_entry(&Entry { name, digest: &digest }).unwrap();
        }
        writer.finish().unwrap();
        std::fs::read(file.path()).unwrap()
    }

    #[test]
    fn links_are_one_per_line() {
        let out = render(Format::Links, &["a b.txt", "c.txt"], LinkOptions::default());
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("ed2k://|file|a%20b.txt|42|"));
        assert!(lines[1].starts_with("ed2k://|file|c.txt|42|"));
    }

    #[test]
    fn unknown_format_is_rejected() {
        assert!("pdf".parse::<Format>().is_err());
    }
}
