//! The Apple System Management Controller: read, write and enumerate its
//! keys, decode their values, and read the fans. Also the wire protocol
//! between Utilify and its root fan helper (`utilify-fanhelper`), since both
//! sides link this crate.
//!
//! Reading works unprivileged through the `AppleSMC` IOKit user client;
//! writing needs root, which is why fan control goes through the helper.
//! Everything hardware-facing sits behind [`SmcIo`], so callers can run
//! against [`fake::FakeSmc`] in tests.

mod client;
pub mod fake;
mod fans;
pub mod protocol;
mod value;

pub use client::Smc;
pub use fans::{FanInfo, fan_count, fan_key, read_fan, read_fans};
pub use value::{decode, encode};

use std::fmt;

/// An SMC key: four ASCII characters packed big-endian, like `F0Ac`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(pub u32);

impl Key {
    /// `None` unless `s` is exactly four ASCII bytes.
    pub fn new(s: &str) -> Option<Key> {
        let b: [u8; 4] = s.as_bytes().try_into().ok()?;
        b.is_ascii().then(|| Key(u32::from_be_bytes(b)))
    }

    pub const fn from_bytes(b: &[u8; 4]) -> Key {
        Key(u32::from_be_bytes(*b))
    }

    pub fn bytes(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.bytes() {
            let c = if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '?'
            };
            write!(f, "{c}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({self})")
    }
}

/// The number of keys (`ui32`).
pub const KEY_COUNT: Key = Key::from_bytes(b"#KEY");
/// The number of fans (`ui8`).
pub const FAN_COUNT: Key = Key::from_bytes(b"FNum");
/// Fan test/force mode (`ui8`). On Apple Silicon it has to be 1 before a
/// fan's manual mode takes effect; other fan tools leave it at 1 while they
/// hold the fans.
pub const FAN_FORCE: Key = Key::from_bytes(b"Ftst");
/// Intel's forced-fans bitmask (`ui16`, bit n = fan n manual).
pub const FAN_FORCE_MASK: Key = Key::from_bytes(b"FS! ");

/// A key's value type, a FourCC like `flt ` or `ui8 `.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DataType(pub u32);

impl DataType {
    pub const FLT: DataType = DataType(u32::from_be_bytes(*b"flt "));
    pub const UI8: DataType = DataType(u32::from_be_bytes(*b"ui8 "));
    pub const UI16: DataType = DataType(u32::from_be_bytes(*b"ui16"));
    pub const UI32: DataType = DataType(u32::from_be_bytes(*b"ui32"));
    pub const SI8: DataType = DataType(u32::from_be_bytes(*b"si8 "));
    pub const SI16: DataType = DataType(u32::from_be_bytes(*b"si16"));
    pub const FLAG: DataType = DataType(u32::from_be_bytes(*b"flag"));
    /// Signed 8.8 fixed point, big-endian (Intel temperatures).
    pub const SP78: DataType = DataType(u32::from_be_bytes(*b"sp78"));
    /// Unsigned 14.2 fixed point, big-endian (Intel fan speeds).
    pub const FPE2: DataType = DataType(u32::from_be_bytes(*b"fpe2"));
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Key(self.0).fmt(f)
    }
}

impl fmt::Debug for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DataType({self})")
    }
}

/// What the SMC says about a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyInfo {
    /// Value size in bytes (at most 32).
    pub size: u32,
    pub data_type: DataType,
    pub attributes: u8,
}

/// A key's raw value with its type.
#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    pub key: Key,
    pub info: KeyInfo,
    pub bytes: Vec<u8>,
}

impl Value {
    /// The value as a number, for the numeric types [`decode`] knows.
    pub fn as_f64(&self) -> Option<f64> {
        decode(self.info.data_type, &self.bytes)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No SMC here (not macOS, or no `AppleSMC` service).
    Unsupported,
    /// The SMC doesn't have this key.
    NotFound(Key),
    /// The kernel refused: writes from a process that isn't root.
    NotPrivileged,
    /// An IOKit call failed with this `kern_return_t`.
    Kernel { call: &'static str, code: i32 },
    /// The SMC answered with a nonzero result code.
    Smc { key: Key, code: u8 },
    /// The value doesn't decode, or can't be encoded, as the key's type.
    BadValue(Key),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unsupported => write!(f, "no SMC on this machine"),
            Error::NotFound(key) => write!(f, "SMC key {key} not found"),
            Error::NotPrivileged => write!(f, "SMC write needs root"),
            Error::Kernel { call, code } => write!(f, "{call} failed: {:#x}", *code as u32),
            Error::Smc { key, code } => write!(f, "SMC error {code:#x} on {key}"),
            Error::BadValue(key) => write!(f, "SMC key {key} has an unexpected value"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Access to SMC keys: the real [`Smc`] or a fake.
pub trait SmcIo {
    fn key_info(&self, key: Key) -> Result<KeyInfo>;
    fn read(&self, key: Key) -> Result<Value>;
    /// Writes `bytes` as the key's value; `bytes` must be the key's size.
    fn write(&self, key: Key, bytes: &[u8]) -> Result<()>;

    /// The key's value as a number.
    fn read_f64(&self, key: Key) -> Result<f64> {
        self.read(key)?.as_f64().ok_or(Error::BadValue(key))
    }

    /// Writes a number, encoded as the key's type.
    fn write_f64(&self, key: Key, v: f64) -> Result<()> {
        let info = self.key_info(key)?;
        let bytes = encode(info, v).ok_or(Error::BadValue(key))?;
        self.write(key, &bytes)
    }

    fn has_key(&self, key: Key) -> bool {
        self.key_info(key).is_ok()
    }
}

impl<T: SmcIo + ?Sized> SmcIo for &T {
    fn key_info(&self, key: Key) -> Result<KeyInfo> {
        (**self).key_info(key)
    }
    fn read(&self, key: Key) -> Result<Value> {
        (**self).read(key)
    }
    fn write(&self, key: Key, bytes: &[u8]) -> Result<()> {
        (**self).write(key, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        let k = Key::new("F0Ac").unwrap();
        assert_eq!(k.0, 0x4630_4163);
        assert_eq!(k.to_string(), "F0Ac");
        assert_eq!(Key::from_bytes(b"#KEY").to_string(), "#KEY");
        assert_eq!(Key::new("F0A"), None);
        assert_eq!(Key::new("F0Acx"), None);
        assert_eq!(Key::new("F0Aé"), None);
        assert_eq!(DataType::FLT.to_string(), "flt ");
    }
}
