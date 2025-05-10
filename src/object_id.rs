use crate::hex;
use blake3::Hash;
use serde::{Deserialize, Serialize};


use std::{
    fmt::{Debug, Display},
    fs::File,
    io::Read,
    path::Path,
};

/// An identifier for a particular piece of binary content.
///
/// This type serves as the core addressing mechanism for the content-addressable storage
/// system. Each file or object in the version control system is identified by its
/// content hash, ensuring unique identification and data integrity.
///
/// Under the hood, this is a [`blake3`] cryptographic hash which provides:
/// - Extremely fast hashing performance
/// - Cryptographically secure identification
/// - Collision resistance (virtually impossible to have two different files with the same hash)
///
/// The identifier is displayed and serialized in hexadecimal format for readability.
///
/// # Examples
///
/// Creating an `ObjectId` from a byte slice:
///
/// ```
/// # fn main() {
/// # // This object is available in this context
/// # struct ObjectId;
/// # impl ObjectId {
/// #    fn from<T>(_: T) -> Self { ObjectId }
/// # }
/// let data = b"Hello, world!";
/// let id = ObjectId::from(data.as_ref());
/// # }
/// ```
///
/// Creating an `ObjectId` from a file:
///
/// ```no_run
/// # fn main() -> Result<(), std::io::Error> {
/// # // This object is available in this context
/// # struct ObjectId;
/// # impl ObjectId {
/// #    fn try_from<T>(_: T) -> Result<Self, std::io::Error> { Ok(ObjectId) }
/// # }
/// use std::path::Path;
///
/// let id = ObjectId::try_from(Path::new("README.md"))?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId(Hash);

impl Serialize for ObjectId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let b: &[u8] = self.0.as_bytes();
        hex::Hex::from(b).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ObjectId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let b: hex::Hex = Deserialize::deserialize(deserializer)?;
        let v: Vec<u8> = b.into();
        let mut bytes: [u8; 32] = [0; 32];
        for i in 0..32 {
            bytes[i] = v[i];
        }
        Ok(ObjectId(Hash::from(bytes)))
    }
}

impl Ord for ObjectId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.as_bytes().cmp(other.0.as_bytes())
    }
}

impl PartialOrd for ObjectId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.0.as_bytes().partial_cmp(other.0.as_bytes())
    }
}

impl Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let b: &[u8] = self.0.as_bytes();
        write!(f, "{}", hex::Hex::from(b))
    }
}

impl Debug for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, f)
    }
}

impl From<&Vec<u8>> for ObjectId {
    fn from(vec: &Vec<u8>) -> Self {
        ObjectId(blake3::hash(&vec))
    }
}

impl From<&[u8]> for ObjectId {
    fn from(bytes: &[u8]) -> Self {
        ObjectId(blake3::hash(&bytes))
    }
}

impl ObjectId {
    /// Create an ObjectId directly from a 32-byte hash value
    /// This is used for reconstructing an ObjectId from a hash string
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        ObjectId(Hash::from(bytes))
    }
}

impl TryFrom<File> for ObjectId {
    type Error = std::io::Error;

    fn try_from(mut f: File) -> Result<Self, Self::Error> {
        let mut vec = Vec::new();
        f.read_to_end(&mut vec)?;
        Ok((&vec).into())
    }
}

impl<'a> TryFrom<&Path> for ObjectId {
    type Error = std::io::Error;

    fn try_from(p: &Path) -> Result<Self, Self::Error> {
        let f = File::options().read(true).open(p)?;
        ObjectId::try_from(f)
    }
}

#[test]
fn test_try_from() -> Result<(), std::io::Error> {
    let object_id = ObjectId::try_from(File::options().read(true).open("./src/lib.rs").unwrap())?;
    let object_id_prime = ObjectId::try_from(Path::new("./src/lib.rs"))?;
    assert_eq!(object_id, object_id_prime);
    Ok(())
}
