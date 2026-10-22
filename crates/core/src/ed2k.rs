//! ed2k (MD4) hash computation from per-part digests.

use md4::{Digest, Md4};

use crate::PART_SIZE;

/// MD4 digest of one part (or of the whole file when it fits in one part).
pub type Md4Digest = [u8; 16];

/// MD4 of a single part of data.
pub fn part_hash(data: &[u8]) -> Md4Digest {
    Md4::digest(data).into()
}

/// Number of part hashes kept for a file of `size` bytes.
///
/// A file whose size is an exact multiple of [`PART_SIZE`] gets one extra
/// trailing hash: the MD4 of an empty block, as in the original ed2k spec.
pub fn part_count(size: u64) -> u64 {
    size / PART_SIZE + 1
}

/// Combines part hashes into the file hash.
///
/// With a single part the file hash is the part hash itself, otherwise it is
/// the MD4 of the concatenated part hashes.
pub fn file_hash(parts: &[Md4Digest]) -> Md4Digest {
    match parts {
        [single] => *single,
        _ => {
            let mut md4 = Md4::new();
            for part in parts {
                md4.update(part);
            }
            md4.finalize().into()
        }
    }
}

/// Lowercase hexadecimal encoding.
pub fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_has_the_well_known_md4() {
        assert_eq!(
            to_hex(&part_hash(b"")),
            "31d6cfe0d16ae931b73c59d7e0c089c0"
        );
    }

    #[test]
    fn rfc1320_abc_vector() {
        assert_eq!(
            to_hex(&part_hash(b"abc")),
            "a448017aaf21d8525fc10ae87aa6729d"
        );
    }

    #[test]
    fn single_part_hash_is_the_part_hash() {
        let part = part_hash(b"hello");
        assert_eq!(file_hash(&[part]), part);
    }

    #[test]
    fn multi_part_hash_is_md4_of_concatenated_hashes() {
        let a = part_hash(b"a");
        let b = part_hash(b"b");
        let mut joined = Vec::new();
        joined.extend_from_slice(&a);
        joined.extend_from_slice(&b);
        assert_eq!(file_hash(&[a, b]), part_hash(&joined));
    }

    #[test]
    fn exact_multiple_gets_an_extra_part() {
        assert_eq!(part_count(0), 1);
        assert_eq!(part_count(1), 1);
        assert_eq!(part_count(PART_SIZE - 1), 1);
        assert_eq!(part_count(PART_SIZE), 2);
        assert_eq!(part_count(PART_SIZE + 1), 2);
        assert_eq!(part_count(PART_SIZE * 3), 4);
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(to_hex(&[0x00, 0xab, 0x0f]), "00ab0f");
    }
}
