use crate::hex;
use blake3::Hash;

use std::{fmt::Display, fs::File, io::Read, path::Path};

/// An identifier for a particular piece of binary content.
/// Under the hood, this is a [`blake3`] hash.
///
/// It is displayed in hexadecimal format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectId(Hash);

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
