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
    /// `<a href>` tags in a minimal HTML page.
    Html,
    /// `[url=ed2k://...]name[/url]` forum tags, one per line.
    UrlTag,
    /// `name,size,link` with a UTF-8 BOM, safe against spreadsheet formulas.
    Csv,
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "links" | "txt" => Ok(Format::Links),
            "html" => Ok(Format::Html),
            "urltag" | "url-tag" | "bbcode" => Ok(Format::UrlTag),
            "csv" => Ok(Format::Csv),
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
    fn unknown_format_is_rejected() {
        assert!("pdf".parse::<Format>().is_err());
    }
}
