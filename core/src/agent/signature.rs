//! Tool-call deduplication via a stable signature.
//!
//! `tool_call_signature(name, input)` returns a short hex digest of
//! `name + "::" + canonical_json(input)`. Two calls hash to the same
//! value iff their tool name and (key-order-independent) arguments
//! match — the agent loop uses this to count repeats and surface
//! "the model is calling the same thing again" in its event stream.
//!
//! A non-cryptographic MD5 implementation is inlined so this module
//! doesn't pull in `md-5` or `digest` just for 16 bytes of fingerprint.

use serde_json::Value;

pub fn tool_call_signature(name: &str, input: &Value) -> String {
    let canonical = canonical_json(input);
    let key = format!("{name}::{canonical}");
    md5_hex_short(key.as_bytes())
}

pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    canonical_write(value, &mut out);
    out
}

fn canonical_write(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('"');
                out.push_str(key);
                out.push_str("\":");
                canonical_write(&map[*key], out);
            }
            out.push('}');
        }
        Value::Array(arr) => {
            out.push('[');
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_write(item, out);
            }
            out.push(']');
        }
        _ => out.push_str(&value.to_string()),
    }
}

fn md5_hex_short(bytes: &[u8]) -> String {
    let digest = md5_compute(bytes);
    let mut s = String::with_capacity(12);
    for byte in &digest[..6] {
        s.push_str(&format!("{:02x}", byte));
    }
    s
}

/// Full 32-hex md5 of the input — stable id derivation (e.g. a conversation's
/// shared trace id), not a security boundary.
pub(crate) fn md5_hex(bytes: &[u8]) -> String {
    let digest = md5_compute(bytes);
    let mut s = String::with_capacity(32);
    for byte in &digest {
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

// RFC 1321 MD5 — non-crypto purpose. Collisions here would mean we
// fail to spot a repeat call, not a security boundary.
fn md5_compute(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    let mut padded = input.to_vec();
    let original_bit_len = (input.len() as u64).wrapping_mul(8);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&original_bit_len.to_le_bytes());

    for chunk in padded.chunks(64) {
        let mut m = [0u32; 16];
        for (i, m_word) in m.iter_mut().enumerate() {
            let start = i * 4;
            *m_word = u32::from_le_bytes([
                chunk[start],
                chunk[start + 1],
                chunk[start + 2],
                chunk[start + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(K[i])
                    .wrapping_add(m[g])
                    .rotate_left(S[i]),
            );
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..].copy_from_slice(&d0.to_le_bytes());
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn signature_stable_across_key_order() {
        let a = tool_call_signature("foo", &json!({"x": 1, "y": [2, 3]}));
        let b = tool_call_signature("foo", &json!({"y": [2, 3], "x": 1}));
        assert_eq!(a, b);
    }

    #[test]
    fn signature_changes_with_input() {
        let a = tool_call_signature("foo", &json!({"x": 1}));
        let b = tool_call_signature("foo", &json!({"x": 2}));
        assert_ne!(a, b);
    }

    #[test]
    fn md5_matches_known_vectors() {
        // RFC 1321 test vectors
        let cases: &[(&str, [u8; 16])] = &[
            (
                "",
                [
                    0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80, 0x09, 0x98, 0xec,
                    0xf8, 0x42, 0x7e,
                ],
            ),
            (
                "abc",
                [
                    0x90, 0x01, 0x50, 0x98, 0x3c, 0xd2, 0x4f, 0xb0, 0xd6, 0x96, 0x3f, 0x7d, 0x28,
                    0xe1, 0x7f, 0x72,
                ],
            ),
            (
                "The quick brown fox jumps over the lazy dog",
                [
                    0x9e, 0x10, 0x7d, 0x9d, 0x37, 0x2b, 0xb6, 0x82, 0x6b, 0xd8, 0x1d, 0x35, 0x42,
                    0xa4, 0x19, 0xd6,
                ],
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(md5_compute(input.as_bytes()), *expected, "md5({input})");
        }
    }
}
