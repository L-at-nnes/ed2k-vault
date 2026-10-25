//! Reading back a saved list of ed2k links.
//!
//! Any text file works (plain links, HTML, URL tags, CSV...): every
//! `ed2k://|file|...|/` link found in it is kept. `.emulecollection` files are
//! read too.

use std::path::Path;

use crate::ed2k::Md4Digest;
use crate::error::{Error, Result};
use crate::link;

/// One file of a saved list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    pub name: String,
    pub size: u64,
    pub hash: Md4Digest,
    /// The link as found in the list (rebuilt for collections).
    pub link: String,
}

const LINK_START: &str = "ed2k://|file|";

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_hash(text: &str) -> Option<Md4Digest> {
    if text.len() != 32 || !text.is_ascii() {
        return None;
    }
    let mut hash = [0u8; 16];
    for (i, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(hash)
}

fn parse_link(link: &str) -> Option<ListEntry> {
    let fields: Vec<&str> = link.split('|').collect();
    if fields.len() < 5 || !fields[1].eq_ignore_ascii_case("file") {
        return None;
    }
    let name = percent_decode(fields[2]);
    let size: u64 = fields[3].parse().ok()?;
    if name.is_empty() {
        return None;
    }
    Some(ListEntry {
        name,
        size,
        hash: parse_hash(fields[4])?,
        link: link.to_string(),
    })
}

/// Every valid file link found in `text`, in order.
pub fn parse_text(text: &str) -> Vec<ListEntry> {
    let lower = text.to_ascii_lowercase();
    let mut entries = Vec::new();
    let mut from = 0;
    while let Some(found) = lower[from..].find(LINK_START) {
        let start = from + found;
        let rest = &text[start..];
        let end = rest
            .find("|/")
            .map(|i| i + 2)
            .or_else(|| rest.find(char::is_whitespace))
            .unwrap_or(rest.len());
        if let Some(entry) = parse_link(&rest[..end]) {
            entries.push(entry);
        }
        from = start + LINK_START.len();
    }
    entries
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| Error::Invalid("truncated collection file".into()))?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn string(&mut self) -> Result<String> {
        let len = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.take(len)?).into_owned())
    }
}

const FT_FILENAME: u8 = 0x01;
const FT_FILESIZE: u8 = 0x02;
const FT_FILEHASH: u8 = 0x28;

#[derive(Default)]
struct Fields {
    name: Option<String>,
    size: Option<u64>,
    hash: Option<Md4Digest>,
}

/// Reads one tag and stores the values we care about in `fields`.
fn read_tag(reader: &mut Reader, fields: &mut Fields) -> Result<()> {
    let kind = reader.u8()?;
    let id = if kind & 0x80 != 0 {
        Some(reader.u8()?)
    } else {
        reader.string()?;
        None
    };
    match kind & 0x7f {
        0x01 => {
            let hash: Md4Digest = reader.take(16)?.try_into().unwrap();
            if id == Some(FT_FILEHASH) {
                fields.hash = Some(hash);
            }
        }
        0x02 => {
            let text = reader.string()?;
            if id == Some(FT_FILENAME) {
                fields.name = Some(text);
            }
        }
        0x03 => {
            let value = reader.u32()? as u64;
            if id == Some(FT_FILESIZE) {
                fields.size = Some(value);
            }
        }
        0x0b => {
            let value = reader.u64()?;
            if id == Some(FT_FILESIZE) {
                fields.size = Some(value);
            }
        }
        0x04 => {
            reader.take(4)?;
        }
        0x05 | 0x09 => {
            reader.take(1)?;
        }
        0x08 => {
            reader.take(2)?;
        }
        other => {
            return Err(Error::Invalid(format!(
                "unsupported tag type {other:#x} in collection file"
            )))
        }
    }
    Ok(())
}

/// Reads an `.emulecollection` file.
pub fn parse_collection(bytes: &[u8]) -> Result<Vec<ListEntry>> {
    let mut reader = Reader { bytes, pos: 0 };
    let version = reader.u32()?;
    if version != 1 && version != 2 {
        return Err(Error::Invalid(format!(
            "unknown collection version {version}"
        )));
    }
    for _ in 0..reader.u32()? {
        read_tag(&mut reader, &mut Fields::default())?;
    }
    let count = reader.u32()?;
    let mut entries = Vec::new();
    for _ in 0..count {
        let mut fields = Fields::default();
        for _ in 0..reader.u32()? {
            read_tag(&mut reader, &mut fields)?;
        }
        if let (Some(name), Some(size), Some(hash)) = (fields.name, fields.size, fields.hash) {
            let link = format!(
                "ed2k://|file|{}|{size}|{}|/",
                link::encode_name(&name),
                crate::ed2k::to_hex(&hash)
            );
            entries.push(ListEntry {
                name,
                size,
                hash,
                link,
            });
        }
    }
    Ok(entries)
}

/// Loads a list of links from a collection or any text file.
pub fn load(path: &Path) -> Result<Vec<ListEntry>> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    let is_collection = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("emulecollection"));
    if is_collection {
        return parse_collection(&bytes);
    }
    let text = String::from_utf8_lossy(&bytes);
    Ok(parse_text(text.trim_start_matches('\u{feff}')))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hasher::FileDigest;
    use crate::link::LinkOptions;
    use crate::output::{self, Entry, Format, Target};

    const HASH: &str = "0123456789abcdef0123456789abcdef";

    fn link(name: &str) -> String {
        format!("ed2k://|file|{name}|1234|{HASH}|/")
    }

    #[test]
    fn plain_links_are_found_and_names_decoded() {
        let text = format!("{}\n{}\n", link("a%20b%C3%A9.txt"), link("plain.bin"));
        let entries = parse_text(&text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "a bé.txt");
        assert_eq!(entries[0].size, 1234);
        assert_eq!(entries[0].hash[0], 0x01);
        assert_eq!(entries[0].link, link("a%20b%C3%A9.txt"));
        assert_eq!(entries[1].name, "plain.bin");
    }

    #[test]
    fn links_inside_html_url_tags_and_csv_are_found() {
        let html = format!("<a href=\"{}\">x</a><br>", link("h.txt"));
        let tag = format!("[url={}]t[/url]", link("t.txt"));
        let csv = format!("\"c.txt\",1234,\"{}\"\r\n", link("c.txt"));
        assert_eq!(parse_text(&html)[0].name, "h.txt");
        assert_eq!(parse_text(&tag)[0].name, "t.txt");
        assert_eq!(parse_text(&csv)[0].name, "c.txt");
    }

    #[test]
    fn extra_fields_do_not_disturb_parsing() {
        let text = format!("ed2k://|file|f|10|{HASH}|h=ABC|p={HASH}:{HASH}|/ and more");
        let entries = parse_text(&text);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].link.ends_with("|/"));
    }

    #[test]
    fn broken_links_are_skipped() {
        let text = format!(
            "ed2k://|file|nosize|abc|{HASH}|/\ned2k://|file|short|10|1234|/\ned2k://|server|1.2.3.4|4661|/\n{}",
            link("ok.txt")
        );
        let entries = parse_text(&text);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "ok.txt");
    }

    #[test]
    fn marker_is_matched_case_insensitively() {
        let text = format!("ED2K://|FILE|Up.txt|5|{HASH}|/");
        assert_eq!(parse_text(&text)[0].name, "Up.txt");
    }

    #[test]
    fn percent_sign_at_the_end_is_kept() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
    }

    #[test]
    fn collection_written_by_us_is_read_back() {
        let file = tempfile::Builder::new()
            .suffix(".emulecollection")
            .tempfile()
            .unwrap();
        let mut writer = output::create(
            Format::Collection,
            Target::File(file.path()),
            LinkOptions {
                hashset: false,
                aich: true,
            },
        )
        .unwrap();
        for (name, size) in [("é one.bin", 42u64), ("big.bin", 5_000_000_000)] {
            let digest = FileDigest {
                size,
                hash: [7; 16],
                part_hashes: vec![[7; 16]],
                aich: Some([1; 20]),
            };
            writer
                .write_entry(&Entry {
                    name,
                    digest: &digest,
                })
                .unwrap();
        }
        writer.finish().unwrap();

        let entries = load(file.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "é one.bin");
        assert_eq!(entries[0].size, 42);
        assert_eq!(entries[0].hash, [7; 16]);
        assert_eq!(entries[1].size, 5_000_000_000);
        assert!(entries[0]
            .link
            .starts_with("ed2k://|file|%C3%A9%20one.bin|42|"));
    }

    #[test]
    fn truncated_or_foreign_collections_are_errors() {
        assert!(parse_collection(&[]).is_err());
        assert!(parse_collection(&[9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_err());
        let mut cut = 2u32.to_le_bytes().to_vec();
        cut.extend_from_slice(&[0; 4]);
        cut.extend_from_slice(&1u32.to_le_bytes());
        cut.extend_from_slice(&3u32.to_le_bytes());
        assert!(parse_collection(&cut).is_err());
    }

    #[test]
    fn text_files_with_a_bom_load() {
        let file = tempfile::Builder::new().suffix(".csv").tempfile().unwrap();
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(link("bom.txt").as_bytes());
        std::fs::write(file.path(), bytes).unwrap();
        assert_eq!(load(file.path()).unwrap()[0].name, "bom.txt");
    }
}
