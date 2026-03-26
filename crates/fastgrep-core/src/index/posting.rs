/// Posting list: varint delta-encoded file ID lists with set operations.

/// Encode a u32 as a varint into a buffer.
pub fn encode_varint(mut value: u32, buf: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Decode a varint from a byte slice, returning (value, bytes_consumed).
pub fn decode_varint(data: &[u8]) -> Option<(u32, usize)> {
    let mut value: u32 = 0;
    let mut shift = 0;
    for (i, &byte) in data.iter().enumerate() {
        value |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
        shift += 7;
        if shift >= 35 {
            return None; // overflow
        }
    }
    None
}

/// Delta-encode a sorted list of file IDs into varint-encoded bytes.
pub fn encode_posting_list(file_ids: &[u32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(file_ids.len() * 2);
    // First: encode count
    encode_varint(file_ids.len() as u32, &mut buf);
    let mut prev = 0u32;
    for &id in file_ids {
        let delta = id - prev;
        encode_varint(delta, &mut buf);
        prev = id;
    }
    buf
}

/// Decode a delta-encoded posting list from bytes.
pub fn decode_posting_list(data: &[u8]) -> Vec<u32> {
    let mut offset = 0;

    // Read count
    let (count, consumed) = match decode_varint(&data[offset..]) {
        Some(v) => v,
        None => return Vec::new(),
    };
    offset += consumed;

    let mut result = Vec::with_capacity(count as usize);
    let mut prev = 0u32;

    for _ in 0..count {
        if offset >= data.len() {
            break;
        }
        let (delta, consumed) = match decode_varint(&data[offset..]) {
            Some(v) => v,
            None => break,
        };
        offset += consumed;
        let id = prev + delta;
        result.push(id);
        prev = id;
    }
    result
}

/// Merge-join intersection of two sorted posting lists.
pub fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                result.push(a[i]);
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    result
}

/// Union of two sorted posting lists.
pub fn union(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                result.push(a[i]);
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => {
                result.push(a[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                result.push(b[j]);
                j += 1;
            }
        }
    }
    result.extend_from_slice(&a[i..]);
    result.extend_from_slice(&b[j..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_roundtrip() {
        for &val in &[0, 1, 127, 128, 300, 16384, u32::MAX] {
            let mut buf = Vec::new();
            encode_varint(val, &mut buf);
            let (decoded, _) = decode_varint(&buf).unwrap();
            assert_eq!(val, decoded);
        }
    }

    #[test]
    fn test_posting_list_roundtrip() {
        let ids = vec![5, 10, 20, 100, 1000];
        let encoded = encode_posting_list(&ids);
        let decoded = decode_posting_list(&encoded);
        assert_eq!(ids, decoded);
    }

    #[test]
    fn test_empty_posting_list() {
        let ids: Vec<u32> = vec![];
        let encoded = encode_posting_list(&ids);
        let decoded = decode_posting_list(&encoded);
        assert_eq!(ids, decoded);
    }

    #[test]
    fn test_intersect() {
        assert_eq!(intersect(&[1, 3, 5, 7], &[2, 3, 5, 8]), vec![3, 5]);
        assert_eq!(intersect(&[1, 2, 3], &[4, 5, 6]), Vec::<u32>::new());
        assert_eq!(intersect(&[1, 2, 3], &[1, 2, 3]), vec![1, 2, 3]);
    }

    #[test]
    fn test_union() {
        assert_eq!(union(&[1, 3, 5], &[2, 3, 6]), vec![1, 2, 3, 5, 6]);
    }
}
