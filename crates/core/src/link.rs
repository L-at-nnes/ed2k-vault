//! ed2k link construction.

use unicode_normalization::UnicodeNormalization;

use crate::aich;
use crate::ed2k::{self, Md4Digest};
use crate::hasher::FileDigest;

/// Optional parts of a link.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkOptions {
    /// Append `p=hash1:hash2:...` (only for files with several parts).
    pub hashset: bool,
    /// Append `h=<AICH root>` when the digest carries one.
    pub aich: bool,
}

/// Percent-encodes a file name: the UTF-8 bytes of everything except ASCII
/// letters, digits and `-_.!~*'()` become `%XX`. The name is normalized to NFC
/// first so macOS (NFD) and other systems produce the same link.
pub fn encode_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.nfc() {
        if c.is_ascii_alphanumeric() || "-_.!~*'()".contains(c) {
            out.push(c);
        } else {
            let mut utf8 = [0u8; 4];
            for byte in c.encode_utf8(&mut utf8).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

/// `ed2k://|file|<name>|<size>|<hash>|[h=<aich>|][p=<hashset>|]/`
pub fn build(name: &str, digest: &FileDigest, options: LinkOptions) -> String {
    let mut link = format!(
        "ed2k://|file|{}|{}|{}|",
        encode_name(name),
        digest.size,
        ed2k::to_hex(&digest.hash)
    );
    if options.aich {
        if let Some(root) = &digest.aich {
            link.push_str("h=");
            link.push_str(&aich::to_base32(root));
            link.push('|');
        }
    }
    if options.hashset && digest.part_hashes.len() > 1 {
        link.push_str("p=");
        let parts: Vec<String> = digest
            .part_hashes
            .iter()
            .map(|part: &Md4Digest| ed2k::to_hex(part))
            .collect();
        link.push_str(&parts.join(":"));
        link.push('|');
    }
    link.push('/');
    link
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(parts: usize, aich: bool) -> FileDigest {
        let part_hashes: Vec<Md4Digest> = (0..parts).map(|i| [i as u8; 16]).collect();
        FileDigest {
            size: 1234,
            hash: [0xab; 16],
            part_hashes,
            aich: aich.then_some([0x11; 20]),
        }
    }

    #[test]
    fn spaces_and_pipes_are_escaped() {
        assert_eq!(encode_name("a b|c"), "a%20b%7Cc");
    }

    #[test]
    fn double_spaces_are_kept() {
        assert_eq!(encode_name("a  b"), "a%20%20b");
    }

    #[test]
    fn accents_become_utf8_escapes() {
        assert_eq!(encode_name("été"), "%C3%A9t%C3%A9");
    }

    #[test]
    fn decomposed_unicode_is_normalized() {
        assert_eq!(encode_name("e\u{301}"), encode_name("\u{e9}"));
    }

    #[test]
    fn unreserved_characters_stay_readable() {
        assert_eq!(encode_name("A-z_0.9!~*'()"), "A-z_0.9!~*'()");
    }

    #[test]
    fn astral_characters_use_four_bytes() {
        assert_eq!(encode_name("\u{1f600}"), "%F0%9F%98%80");
    }

    #[test]
    fn minimal_link() {
        let link = build("f.bin", &digest(1, true), LinkOptions::default());
        assert_eq!(
            link,
            format!("ed2k://|file|f.bin|1234|{}|/", "ab".repeat(16))
        );
    }

    #[test]
    fn aich_comes_before_the_hashset() {
        let options = LinkOptions {
            hashset: true,
            aich: true,
        };
        let link = build("f.bin", &digest(2, true), options);
        let hash = "ab".repeat(16);
        let root = aich::to_base32(&[0x11; 20]);
        let set = format!("{}:{}", "00".repeat(16), "01".repeat(16));
        assert_eq!(
            link,
            format!("ed2k://|file|f.bin|1234|{hash}|h={root}|p={set}|/")
        );
    }

    #[test]
    fn single_part_files_have_no_hashset() {
        let options = LinkOptions {
            hashset: true,
            aich: false,
        };
        assert!(!build("f", &digest(1, false), options).contains("p="));
    }

    #[test]
    fn missing_aich_is_skipped() {
        let options = LinkOptions {
            hashset: false,
            aich: true,
        };
        assert!(!build("f", &digest(1, false), options).contains("h="));
    }
}
