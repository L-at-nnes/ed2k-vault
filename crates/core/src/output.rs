//! Streaming output writers: every entry is written as soon as it is hashed.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::str::FromStr;

use crate::aich;
use crate::error::{Error, Result};
use crate::hasher::FileDigest;
use crate::link::{self, LinkOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// One link per line.
    Links,
    /// `<a href>` tags in a minimal HTML page.
    Html,
    /// `[url=ed2k://...]name[/url]` forum tags, one per line.
    UrlTag,
    /// `name,size,link` with a UTF-8 BOM, safe against spreadsheet formulas.
    Csv,
    /// Binary `.emulecollection` file (needs a real output file).
    Collection,
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "links" | "txt" => Ok(Format::Links),
            "html" => Ok(Format::Html),
            "urltag" | "url-tag" | "bbcode" => Ok(Format::UrlTag),
            "csv" => Ok(Format::Csv),
            "collection" | "emulecollection" => Ok(Format::Collection),
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

const HTML_HEADER: &str = "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>ed2k links</title>\n</head>\n<body>\n";
const HTML_FOOTER: &str = "</body>\n</html>\n";

/// Opens `target` and returns the writer for `format`.
pub fn create(
    format: Format,
    target: Target,
    options: LinkOptions,
) -> Result<Box<dyn EntryWriter>> {
    if format == Format::Collection {
        let Target::File(path) = target else {
            return Err(Error::Invalid(
                "the collection format needs an output file".into(),
            ));
        };
        let file = File::create(path).map_err(|e| Error::io(path, e))?;
        return Ok(Box::new(
            CollectionWriter::new(file, options).map_err(|e| Error::io(path, e))?,
        ));
    }
    let out: Box<dyn Write + Send> = match target {
        Target::File(path) => {
            let file = File::create(path).map_err(|e| Error::io(path, e))?;
            Box::new(BufWriter::with_capacity(BUFFER_SIZE, file))
        }
        Target::Stdout => Box::new(BufWriter::new(io::stdout())),
    };
    let style = match format {
        Format::Links => Style::Links,
        Format::Html => Style::Html,
        Format::UrlTag => Style::UrlTag,
        Format::Csv => Style::Csv,
        Format::Collection => unreachable!("handled above"),
    };
    let mut writer = TextWriter {
        out,
        options,
        style,
    };
    writer
        .write_header()
        .map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(Box::new(writer))
}

#[derive(Clone, Copy)]
enum Style {
    Links,
    Html,
    UrlTag,
    Csv,
}

struct TextWriter {
    out: Box<dyn Write + Send>,
    options: LinkOptions,
    style: Style,
}

impl TextWriter {
    fn write_header(&mut self) -> io::Result<()> {
        match self.style {
            Style::Links | Style::UrlTag => Ok(()),
            Style::Html => self.out.write_all(HTML_HEADER.as_bytes()),
            Style::Csv => self.out.write_all(b"\xEF\xBB\xBFname,size,link\r\n"),
        }
    }
}

/// Quotes a CSV cell; a leading `= + - @` or control character is defused
/// with a `'` prefix so spreadsheets never evaluate it.
fn csv_cell(text: &str) -> String {
    let mut cell = String::with_capacity(text.len() + 2);
    if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        cell.push('\'');
    }
    cell.push_str(text);
    format!("\"{}\"", cell.replace('"', "\"\""))
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

impl EntryWriter for TextWriter {
    fn write_entry(&mut self, entry: &Entry) -> io::Result<()> {
        let link = link::build(entry.name, entry.digest, self.options);
        match self.style {
            Style::Links => writeln!(self.out, "{link}"),
            Style::Html => writeln!(
                self.out,
                "<a href=\"{link}\">{}</a><br>",
                escape_html(entry.name)
            ),
            Style::UrlTag => writeln!(self.out, "[url={link}]{}[/url]", entry.name),
            Style::Csv => write!(
                self.out,
                "{},{},{}\r\n",
                csv_cell(entry.name),
                entry.digest.size,
                csv_cell(&link)
            ),
        }
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        if matches!(self.style, Style::Html) {
            self.out.write_all(HTML_FOOTER.as_bytes())?;
        }
        self.out.flush()
    }
}

const COLLECTION_VERSION: u32 = 2;
const TAG_HASH: u8 = 0x81;
const TAG_STRING: u8 = 0x82;
const TAG_UINT32: u8 = 0x83;
const TAG_UINT64: u8 = 0x8b;
const FT_FILENAME: u8 = 0x01;
const FT_FILESIZE: u8 = 0x02;
const FT_AICH_FILEHASH: u8 = 0x27;
const FT_FILEHASH: u8 = 0x28;
/// Byte offset of the entry count in the header (version + header tag count).
const COUNT_OFFSET: u64 = 8;

/// eMule collection: the entry count lives in the header, so it is written as
/// a placeholder and patched in `finish`.
struct CollectionWriter {
    out: BufWriter<File>,
    options: LinkOptions,
    count: u32,
}

impl CollectionWriter {
    fn new(file: File, options: LinkOptions) -> io::Result<Self> {
        let mut out = BufWriter::with_capacity(BUFFER_SIZE, file);
        out.write_all(&COLLECTION_VERSION.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
        Ok(Self {
            out,
            options,
            count: 0,
        })
    }

    fn write_string(&mut self, tag: u8, text: &str) -> io::Result<()> {
        let mut end = text.len().min(u16::MAX as usize);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.out.write_all(&[TAG_STRING, tag])?;
        self.out.write_all(&(end as u16).to_le_bytes())?;
        self.out.write_all(&text.as_bytes()[..end])
    }
}

impl EntryWriter for CollectionWriter {
    fn write_entry(&mut self, entry: &Entry) -> io::Result<()> {
        let digest = entry.digest;
        let aich = self
            .options
            .aich
            .then_some(digest.aich)
            .flatten()
            .map(|root| aich::to_base32(&root));
        let tags = if aich.is_some() { 4u32 } else { 3 };
        self.out.write_all(&tags.to_le_bytes())?;

        self.out.write_all(&[TAG_HASH, FT_FILEHASH])?;
        self.out.write_all(&digest.hash)?;

        if digest.size > u32::MAX as u64 {
            self.out.write_all(&[TAG_UINT64, FT_FILESIZE])?;
            self.out.write_all(&digest.size.to_le_bytes())?;
        } else {
            self.out.write_all(&[TAG_UINT32, FT_FILESIZE])?;
            self.out.write_all(&(digest.size as u32).to_le_bytes())?;
        }

        self.write_string(FT_FILENAME, entry.name)?;
        if let Some(root) = aich {
            self.write_string(FT_AICH_FILEHASH, &root)?;
        }
        self.count += 1;
        Ok(())
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        let mut file = self.out.into_inner().map_err(|e| e.into_error())?;
        file.seek(SeekFrom::Start(COUNT_OFFSET))?;
        file.write_all(&self.count.to_le_bytes())?;
        file.flush()
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
            writer
                .write_entry(&Entry {
                    name,
                    digest: &digest,
                })
                .unwrap();
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
    fn html_wraps_links_in_anchors_and_escapes_names() {
        let out = render(Format::Html, &["a&b<c>.txt"], LinkOptions::default());
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("<!DOCTYPE html>"));
        assert!(text.contains("<a href=\"ed2k://|file|a%26b%3Cc%3E.txt|42|"));
        assert!(text.contains(">a&amp;b&lt;c&gt;.txt</a><br>"));
        assert!(text.trim_end().ends_with("</html>"));
    }

    #[test]
    fn url_tags_show_the_plain_name() {
        let out = render(Format::UrlTag, &["a b.txt"], LinkOptions::default());
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("[url=ed2k://|file|a%20b.txt|42|"));
        assert!(text.trim_end().ends_with("|/]a b.txt[/url]"));
    }

    #[test]
    fn csv_has_bom_header_and_quoted_cells() {
        let out = render(Format::Csv, &["a \"q\", b.txt"], LinkOptions::default());
        assert!(out.starts_with(b"\xEF\xBB\xBFname,size,link\r\n"));
        let text = String::from_utf8(out[3..].to_vec()).unwrap();
        let row = text.lines().nth(1).unwrap();
        assert!(row.starts_with("\"a \"\"q\"\", b.txt\",42,\"ed2k://"));
    }

    #[test]
    fn csv_defuses_spreadsheet_formulas() {
        let out = render(Format::Csv, &["=HYPERLINK(1)"], LinkOptions::default());
        let text = String::from_utf8(out[3..].to_vec()).unwrap();
        assert!(text.contains("\"'=HYPERLINK(1)\""));
    }

    #[test]
    fn collection_layout_matches_the_emule_format() {
        let options = LinkOptions {
            hashset: false,
            aich: true,
        };
        let out = render(Format::Collection, &["é.bin"], options);
        let mut expected = Vec::new();
        expected.extend_from_slice(&2u32.to_le_bytes()); // version
        expected.extend_from_slice(&0u32.to_le_bytes()); // header tags
        expected.extend_from_slice(&1u32.to_le_bytes()); // entries
        expected.extend_from_slice(&4u32.to_le_bytes()); // tags of the entry
        expected.extend_from_slice(&[0x81, 0x28]);
        expected.extend_from_slice(&[1; 16]);
        expected.extend_from_slice(&[0x83, 0x02]);
        expected.extend_from_slice(&42u32.to_le_bytes());
        expected.extend_from_slice(&[0x82, 0x01]);
        expected.extend_from_slice(&6u16.to_le_bytes());
        expected.extend_from_slice("é.bin".as_bytes());
        expected.extend_from_slice(&[0x82, 0x27]);
        expected.extend_from_slice(&32u16.to_le_bytes());
        expected.extend_from_slice(aich::to_base32(&[2; 20]).as_bytes());
        assert_eq!(out, expected);
    }

    #[test]
    fn collection_without_aich_has_three_tags_and_large_sizes_use_64_bits() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = create(
            Format::Collection,
            Target::File(file.path()),
            LinkOptions::default(),
        )
        .unwrap();
        let mut digest = digest();
        digest.size = 5_000_000_000;
        writer
            .write_entry(&Entry {
                name: "big",
                digest: &digest,
            })
            .unwrap();
        writer.finish().unwrap();
        let out = std::fs::read(file.path()).unwrap();
        assert_eq!(&out[12..16], &3u32.to_le_bytes());
        assert_eq!(&out[34..36], &[0x8b, 0x02]);
        assert_eq!(&out[36..44], &5_000_000_000u64.to_le_bytes());
    }

    #[test]
    fn collection_cannot_go_to_stdout() {
        assert!(create(Format::Collection, Target::Stdout, LinkOptions::default()).is_err());
    }

    #[test]
    fn unknown_format_is_rejected() {
        assert!("pdf".parse::<Format>().is_err());
    }
}
