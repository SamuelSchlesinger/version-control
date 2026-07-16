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
        format!("{self}").serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: String = String::deserialize(deserializer)?;
        let bytes = s.into_bytes();
        // Uphold the "valid hexadecimal encoding" invariant here, at the trust
        // boundary, rather than panicking later during decode. Input arrives
        // from the network, so a malformed string must be an error, not a crash.
        if bytes.len() % 2 != 0 {
            return Err(serde::de::Error::custom("hex string has odd length"));
        }
        if let Some(&bad) = bytes.iter().find(|&&b| !b.is_ascii_hexdigit()) {
            return Err(serde::de::Error::custom(format!(
                "invalid hex digit: {:?}",
                bad as char
            )));
        }
        Ok(Hex(bytes))
    }
}

impl Display for Hex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let r: &[u8] = &self.0;
        write!(f, "{}", std::str::from_utf8(r).expect("Hex always contains valid UTF-8"))
    }
}

impl From<&[u8]> for Hex {
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

impl Hex {
    /// Decodes the hex text into bytes.
    ///
    /// Returns an error instead of panicking on odd length or a non-hex digit,
    /// so untrusted input (e.g. an object id from the network) can never crash
    /// the process. Accepts both lower- and upper-case digits.
    pub fn decode(&self) -> Result<Vec<u8>, String> {
        fn unhex_digit(h: u8) -> Result<u8, String> {
            match h {
                b'0'..=b'9' => Ok(h - b'0'),
                b'a'..=b'f' => Ok(h - b'a' + 10),
                b'A'..=b'F' => Ok(h - b'A' + 10),
                _ => Err(format!("invalid hex digit: {:?}", h as char)),
            }
        }
        let n = self.0.len();
        if n % 2 != 0 {
            return Err("hex length is not even".to_string());
        }

        let mut v = vec![0u8; n / 2];
        for (i, item) in v.iter_mut().enumerate() {
            let j = i * 2;
            *item |= unhex_digit(self.0[j])? << 4;
            *item |= unhex_digit(self.0[j + 1])?;
        }
        Ok(v)
    }
}

impl TryFrom<Hex> for Vec<u8> {
    type Error = String;

    fn try_from(value: Hex) -> Result<Self, Self::Error> {
        value.decode()
    }
}

#[test]
fn test_hex_round_trip() {
    let example: &[u8] = b"hello, world";
    let hex: Hex = Hex::from(example);
    let bytes: Vec<u8> = hex.decode().unwrap();
    let bytes_ref: &[u8] = &bytes;
    assert_eq!(example, bytes_ref);
}

#[test]
fn test_hex_decode_rejects_bad_input() {
    // Uppercase is accepted (round-trips to the same bytes as lowercase).
    assert_eq!(Hex(b"AABB".to_vec()).decode().unwrap(), vec![0xaa, 0xbb]);
    // Odd length and non-hex digits are errors, never panics.
    assert!(Hex(b"abc".to_vec()).decode().is_err());
    assert!(Hex(b"zz".to_vec()).decode().is_err());
}

#[test]
fn test_hex_deserialize_rejects_malformed() {
    // The trust boundary: a malformed string must not deserialize into a Hex.
    assert!(serde_json::from_str::<Hex>("\"zzzz\"").is_err());
    assert!(serde_json::from_str::<Hex>("\"abc\"").is_err());
    assert!(serde_json::from_str::<Hex>("\"aabb\"").is_ok());
}

#[test]
fn test_hex_deserialize() {
    let example: &[u8] = b"hello, world";
    let hex: Hex = Hex::from(example);
    let json = serde_json::to_vec(&hex).unwrap();
    let hex_: Hex = serde_json::from_slice(&json).unwrap();
    assert_eq!(hex, hex_);
}
