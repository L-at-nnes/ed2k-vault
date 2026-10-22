//! AICH (Advanced Intelligent Corruption Handling) root hash.
//!
//! The tree leaves are SHA-1 digests of [`AICH_BLOCK_SIZE`] blocks. Nodes are
//! split the way eMule does: a node spanning more than one part is split on
//! part boundaries, a node inside a part on block boundaries, and the left
//! child of a left branch takes the larger half.
//!
//! Blocks never straddle two parts: [`PART_SIZE`] is not a multiple of
//! [`AICH_BLOCK_SIZE`], so the last block of every full part is shorter.

use sha1::{Digest, Sha1};

use crate::{AICH_BLOCK_SIZE, PART_SIZE};

pub type Sha1Digest = [u8; 20];

/// SHA-1 of one AICH block.
pub fn block_hash(data: &[u8]) -> Sha1Digest {
    Sha1::digest(data).into()
}

/// Number of blocks in a full part.
pub const BLOCKS_PER_PART: u64 = PART_SIZE.div_ceil(AICH_BLOCK_SIZE);

/// Number of block digests expected for a file of `size` bytes.
pub fn block_count(size: u64) -> u64 {
    let rest = size % PART_SIZE;
    size / PART_SIZE * BLOCKS_PER_PART + rest.div_ceil(AICH_BLOCK_SIZE)
}

/// SHA-1 digests of every block of one part (or file tail), in order.
pub fn block_hashes(data: &[u8]) -> Vec<Sha1Digest> {
    data.chunks(AICH_BLOCK_SIZE as usize)
        .map(block_hash)
        .collect()
}

/// Root hash of the tree whose leaves are `blocks`, for a file of `size` bytes.
///
/// `blocks` holds the digests of each part's blocks, part after part (see
/// [`block_count`]). An empty file has no block: its root is the SHA-1 of
/// nothing.
pub fn root_hash(blocks: &[Sha1Digest], size: u64) -> Sha1Digest {
    if size == 0 {
        return block_hash(b"");
    }
    debug_assert_eq!(blocks.len() as u64, block_count(size));
    node_hash(blocks, 0, size, true)
}

fn node_hash(blocks: &[Sha1Digest], offset: u64, size: u64, is_left: bool) -> Sha1Digest {
    if size <= AICH_BLOCK_SIZE {
        let in_part = offset % PART_SIZE;
        let index = offset / PART_SIZE * BLOCKS_PER_PART + in_part / AICH_BLOCK_SIZE;
        return blocks[index as usize];
    }
    let base = if size <= PART_SIZE {
        AICH_BLOCK_SIZE
    } else {
        PART_SIZE
    };
    let units = size.div_ceil(base);
    let left_units = if is_left { units + 1 } else { units } / 2;
    let left_size = left_units * base;
    let left = node_hash(blocks, offset, left_size, true);
    let right = node_hash(blocks, offset + left_size, size - left_size, false);
    let mut sha1 = Sha1::new();
    sha1.update(left);
    sha1.update(right);
    sha1.finalize().into()
}

/// RFC 4648 Base32 without padding (a 20-byte digest gives 32 characters).
pub fn to_base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer = 0u32;
    let mut bits = 0;
    for &byte in bytes {
        buffer = (buffer << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(a: &Sha1Digest, b: &Sha1Digest) -> Sha1Digest {
        let mut joined = Vec::new();
        joined.extend_from_slice(a);
        joined.extend_from_slice(b);
        block_hash(&joined)
    }

    fn data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    #[test]
    fn base32_matches_known_sha1_of_abc() {
        assert_eq!(
            to_base32(&block_hash(b"abc")),
            "VGMT4NSHA2AWVOR6EVYXQUGCNSONBWE5"
        );
    }

    #[test]
    fn base32_has_32_chars_for_a_sha1() {
        assert_eq!(to_base32(&[0u8; 20]).len(), 32);
    }

    #[test]
    fn small_file_root_is_plain_sha1() {
        let bytes = data(1000);
        let blocks = block_hashes(&bytes);
        assert_eq!(root_hash(&blocks, 1000), block_hash(&bytes));
    }

    #[test]
    fn empty_file_root_is_sha1_of_nothing() {
        assert_eq!(root_hash(&[], 0), block_hash(b""));
    }

    #[test]
    fn two_blocks_are_paired() {
        let bytes = data(AICH_BLOCK_SIZE as usize + 10);
        let b = block_hashes(&bytes);
        assert_eq!(root_hash(&b, bytes.len() as u64), pair(&b[0], &b[1]));
    }

    #[test]
    fn three_blocks_give_the_left_branch_two_blocks() {
        let bytes = data(AICH_BLOCK_SIZE as usize * 2 + 10);
        let b = block_hashes(&bytes);
        let expected = pair(&pair(&b[0], &b[1]), &b[2]);
        assert_eq!(root_hash(&b, bytes.len() as u64), expected);
    }

    #[test]
    fn five_blocks_follow_the_ed2k_split() {
        // root (left branch): 3 blocks left, 2 right.
        let bytes = data(AICH_BLOCK_SIZE as usize * 4 + 10);
        let b = block_hashes(&bytes);
        let left = pair(&pair(&b[0], &b[1]), &b[2]);
        let right = pair(&b[3], &b[4]);
        assert_eq!(root_hash(&b, bytes.len() as u64), pair(&left, &right));
    }

    #[test]
    fn block_count_accounts_for_short_last_block_of_each_part() {
        assert_eq!(BLOCKS_PER_PART, 53);
        assert_eq!(block_count(1), 1);
        assert_eq!(block_count(PART_SIZE), 53);
        assert_eq!(block_count(PART_SIZE + 1), 54);
        assert_eq!(block_count(PART_SIZE * 2), 106);
    }

    #[test]
    fn nodes_spanning_parts_split_on_part_boundaries() {
        // 2 full parts + a 3-block tail: the root has 3 part units, the left
        // node holds the first two parts and the right node is the tail.
        let size = PART_SIZE * 2 + AICH_BLOCK_SIZE * 2 + 10;
        let total = block_count(size) as usize;
        let blocks: Vec<Sha1Digest> = (0..total).map(|i| block_hash(&[i as u8])).collect();
        let part = |index: u64, is_left: bool| {
            node_hash(&blocks, index * PART_SIZE, PART_SIZE, is_left)
        };
        let left = pair(&part(0, true), &part(1, false));
        let tail = node_hash(&blocks, PART_SIZE * 2, size - PART_SIZE * 2, false);
        assert_eq!(root_hash(&blocks, size), pair(&left, &tail));
        // the tail's blocks start right after the 2 * 53 blocks of full parts
        let tail_blocks = &blocks[106..];
        assert_eq!(tail_blocks.len(), 3);
        let expected_tail = pair(&tail_blocks[0], &pair(&tail_blocks[1], &tail_blocks[2]));
        assert_eq!(tail, expected_tail);
    }
}
