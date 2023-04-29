use std::fmt::Display;

use serde::{Deserialize, Serialize};

/// A valid hexadecimal encoding of binary data.
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct Hex(pub Vec<u8>);

impl Serialize for Hex {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        format!("{}", self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: String = String::deserialize(deserializer)?;
        let b: Vec<u8> = s.into_bytes().iter().copied().collect();
        Ok(Hex(b))
    }
}

impl Display for Hex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let r: &[u8] = &self.0;
        write!(f, "{}", std::str::from_utf8(r).unwrap())
    }
}

impl<'a> From<&'a [u8]> for Hex {
    fn from(bytes: &[u8]) -> Self {
        fn hex_digit(b: u8) -> u8 {
            if b <= 9 {
                b + b'0'
            } else if b < 16 {
                b + b'a' - 10
            } else {
                unreachable!("bad hex digit")
            }
        }

        let mut out = vec![0u8; bytes.len() * 2];
        let mut i = 0;
        for &b in bytes {
            out[i] = hex_digit((b & 0b11110000) >> 4);
            out[i + 1] = hex_digit(b & 0b00001111);
            i += 2;
        }
        Hex(out)
    }
}

impl From<Hex> for Vec<u8> {
    fn from(value: Hex) -> Self {
        fn unhex_digit(h: u8) -> u8 {
            if h >= b'0' && h <= b'9' {
                h - b'0'
            } else if h >= b'a' && h <= b'f' {
                h - b'a' + 10
            } else {
                unreachable!("bad hex undigit: {}", h)
            }
        }
        let n = value.0.len();

        if n % 2 != 0 {
            unreachable!("hex length is not even");
        }

        let mut v = vec![0u8; n / 2];

        for i in 0..(n / 2) {
            let j = i * 2;
            v[i] |= unhex_digit(value.0[j]) << 4;
            v[i] |= unhex_digit(value.0[j + 1]);
        }

        v
    }
}

#[test]
fn test_hex_round_trip() {
    let example: &[u8] = b"hello, world";
    let hex: Hex = Hex::from(example);
    let bytes: Vec<u8> = hex.into();
    let bytes_ref: &[u8] = &bytes;
    assert_eq!(example, bytes_ref);
}

#[test]
fn test_hex_deserialize() {
    let example: &[u8] = b"hello, world";
    let hex: Hex = Hex::from(example);
    let json = serde_json::to_vec(&hex).unwrap();
    let hex_: Hex = serde_json::from_slice(&json).unwrap();
    assert_eq!(hex, hex_);
}
