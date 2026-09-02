//! Low-level classic (non-Big) TIFF byte encoding: one IFD's tag entries
//! plus whatever external (>4 byte) tag data they carry. Used by
//! [`crate::author`] to write real COG output.
//!
//! Hand-rolled, not the `tiff` crate's own `encoder` module: that module's
//! high-level `ImageEncoder` only ever writes strips (`write_strip`/
//! `rows_per_strip`) — there is no tiled-output API to build a COG's tiled
//! IFDs from — and its lower-level `DirectoryEncoder` is shaped around
//! writing one IFD's tag data sequentially as it goes, which doesn't fit
//! [`crate::author`]'s own header-first file layout (every IFD placed
//! before any tile's pixel data, so a range-reading client's own read-ahead
//! window can pick up every level's tags in as few requests as possible —
//! see `author.rs`'s own doc). This crate already carries the equivalent
//! technique for a fixture-only purpose (`examples/gen_fixture.rs`'s own
//! `Value`/`Tag`/`encode_ifd`); this is that same TIFF6 §2 byte-layout
//! knowledge, written fresh here for the real authoring feature rather than
//! reused from an example file the library crate doesn't build against.

/// One tag's value, widened to the three TIFF field types this crate's
/// authoring output ever needs (SHORT, LONG, DOUBLE) — covers every tag
/// [`crate::author`] writes: dimensions/compression/photometric (SHORT or
/// LONG), tile offsets/byte counts (LONG), and the georeferencing tags
/// (DOUBLE, plus the GeoKey directory itself as SHORT).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Short(Vec<u16>),
    Long(Vec<u32>),
    Double(Vec<f64>),
}

impl Value {
    fn type_code(&self) -> u16 {
        match self {
            Value::Short(_) => 3,
            Value::Long(_) => 4,
            Value::Double(_) => 12,
        }
    }

    fn count(&self) -> u32 {
        match self {
            Value::Short(v) => v.len() as u32,
            Value::Long(v) => v.len() as u32,
            Value::Double(v) => v.len() as u32,
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Value::Short(v) => {
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
            Value::Long(v) => {
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
            Value::Double(v) => {
                for x in v {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
        }
        out
    }
}

/// One (tag id, value) pair. Callers MUST supply entries already sorted
/// ascending by tag id — the order TIFF6 §2 requires an IFD's entries to
/// appear in; this module does not sort for them (same contract
/// `examples/gen_fixture.rs` already documents for its own `encode_ifd`).
pub(crate) type TagEntry = (u16, Value);

/// The byte size of `tags` encoded as one IFD: the entry count (2 bytes),
/// 12 bytes per entry, the "next IFD offset" field (4 bytes), plus every
/// entry whose value doesn't fit inline (> 4 bytes) stored immediately
/// after, back to back. Every value type this module ever writes (SHORT,
/// LONG, DOUBLE) is an even byte count, so no padding is ever needed to
/// keep an external value's own start word-aligned.
pub(crate) fn ifd_encoded_size(tags: &[TagEntry]) -> u32 {
    let mut size = 2 + 12 * tags.len() as u32 + 4;
    for (_, value) in tags {
        let len = value.bytes().len();
        if len > 4 {
            size += len as u32;
        }
    }
    size
}

/// Encodes one IFD at `base_offset` (its absolute position in the file):
/// entry count, then one 12-byte entry per tag (a value > 4 bytes points at
/// an offset into the external data that follows every entry), then
/// `next_ifd_offset` (0 ends the IFD chain), then the external data itself.
pub(crate) fn encode_ifd(tags: &[TagEntry], next_ifd_offset: u32, base_offset: u32) -> Vec<u8> {
    let mut entries = Vec::new();
    let mut external = Vec::new();
    let external_start = base_offset + 2 + 12 * tags.len() as u32 + 4;

    for (tag_id, value) in tags {
        entries.extend_from_slice(&tag_id.to_le_bytes());
        entries.extend_from_slice(&value.type_code().to_le_bytes());
        entries.extend_from_slice(&value.count().to_le_bytes());
        let bytes = value.bytes();
        if bytes.len() <= 4 {
            let mut inline = bytes.clone();
            inline.resize(4, 0);
            entries.extend_from_slice(&inline);
        } else {
            let offset = external_start + external.len() as u32;
            entries.extend_from_slice(&offset.to_le_bytes());
            external.extend_from_slice(&bytes);
        }
    }

    let mut out = Vec::with_capacity(2 + entries.len() + 4 + external.len());
    out.extend_from_slice(&(tags.len() as u16).to_le_bytes());
    out.extend_from_slice(&entries);
    out.extend_from_slice(&next_ifd_offset.to_le_bytes());
    out.extend_from_slice(&external);
    out
}

/// The 8-byte classic TIFF header: little-endian byte order mark, magic
/// number 42, and the first IFD's absolute file offset.
pub(crate) fn tiff_header(first_ifd_offset: u32) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[0..2].copy_from_slice(b"II");
    out[2..4].copy_from_slice(&42u16.to_le_bytes());
    out[4..8].copy_from_slice(&first_ifd_offset.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiff_header_is_little_endian_classic_tiff() {
        let header = tiff_header(0x0000_0010);
        assert_eq!(&header[0..2], b"II");
        assert_eq!(&header[2..4], &42u16.to_le_bytes());
        assert_eq!(&header[4..8], &0x0000_0010u32.to_le_bytes());
    }

    #[test]
    fn ifd_encoded_size_accounts_for_inline_and_external_values() {
        let tags: Vec<TagEntry> = vec![
            (256, Value::Short(vec![100])),       // inline (2 bytes)
            (324, Value::Long(vec![1, 2, 3, 4])), // external (16 bytes)
        ];
        // header(2) + 2*12(entries) + next_ifd(4) + external(16)
        assert_eq!(ifd_encoded_size(&tags), 2 + 24 + 4 + 16);
    }

    #[test]
    fn encode_ifd_round_trips_entry_count_and_next_ifd_offset() {
        let tags: Vec<TagEntry> = vec![(256, Value::Short(vec![256]))];
        let encoded = encode_ifd(&tags, 0x1234, 8);
        let entry_count = u16::from_le_bytes([encoded[0], encoded[1]]);
        assert_eq!(entry_count, 1);
        // entry_count(2) + one 12-byte entry(tag(2)+type(2)+count(4)+value(4)).
        let next_ifd_offset = u32::from_le_bytes(encoded[14..18].try_into().unwrap());
        assert_eq!(next_ifd_offset, 0x1234);
    }

    #[test]
    fn encode_ifd_points_an_external_value_at_the_correct_absolute_offset() {
        let tags: Vec<TagEntry> = vec![(324, Value::Long(vec![10, 20, 30]))];
        let base_offset = 100;
        let encoded = encode_ifd(&tags, 0, base_offset);
        // entry_count(2) + tag(2) + type(2) + count(4) = 10 bytes before the
        // value/offset field itself (4 bytes).
        let value_offset = u32::from_le_bytes(encoded[10..14].try_into().unwrap());
        let external_start = base_offset + 2 + 12 + 4;
        assert_eq!(value_offset, external_start);
        let external_bytes = &encoded[(2 + 12 + 4) as usize..];
        assert_eq!(
            external_bytes,
            &[10u32, 20, 30]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>()[..]
        );
    }

    #[test]
    fn encode_ifd_writes_a_small_value_inline_not_external() {
        let tags: Vec<TagEntry> = vec![(256, Value::Short(vec![42]))];
        let encoded = encode_ifd(&tags, 0, 8);
        // No external data: total length is exactly header+entry+next_ifd.
        assert_eq!(encoded.len(), 2 + 12 + 4);
        // entry_count(2) + tag(2) + type(2) + count(4) = 10 bytes before the
        // inline value field itself.
        assert_eq!(&encoded[10..12], &42u16.to_le_bytes());
    }
}
