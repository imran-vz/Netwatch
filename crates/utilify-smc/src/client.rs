//! The real SMC, through the `AppleSMC` IOKit user client: struct method 2
//! with the classic 80-byte `SMCKeyData`, its `data8` field picking the
//! command.

use crate::{DataType, Error, Key, KeyInfo, Result, SmcIo, Value};

/// An open connection to the SMC. Closed on drop.
pub struct Smc {
    #[cfg(target_os = "macos")]
    conn: u32,
}

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::{c_char, c_void};

    pub const KERNEL_INDEX_SMC: u32 = 2;
    pub const CMD_READ_BYTES: u8 = 5;
    pub const CMD_WRITE_BYTES: u8 = 6;
    pub const CMD_READ_INDEX: u8 = 8;
    pub const CMD_READ_KEYINFO: u8 = 9;
    /// `kSMCKeyNotFound`.
    pub const SMC_KEY_NOT_FOUND: u8 = 0x84;
    /// `kIOReturnNotPrivileged`.
    pub const IO_RETURN_NOT_PRIVILEGED: i32 = 0xe000_02c1_u32 as i32;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Vers {
        pub major: u8,
        pub minor: u8,
        pub build: u8,
        pub reserved: u8,
        pub release: u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct PLimit {
        pub version: u16,
        pub length: u16,
        pub cpu: u32,
        pub gpu: u32,
        pub mem: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct KeyInfo {
        pub data_size: u32,
        pub data_type: u32,
        pub attributes: u8,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct KeyData {
        pub key: u32,
        pub vers: Vers,
        pub p_limit: PLimit,
        pub key_info: KeyInfo,
        pub result: u8,
        pub status: u8,
        pub data8: u8,
        pub data32: u32,
        pub bytes: [u8; 32],
    }

    const _: () = assert!(std::mem::size_of::<KeyData>() == 80);

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        pub fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        pub fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> u32;
        pub fn IOServiceOpen(service: u32, owning_task: u32, kind: u32, conn: *mut u32) -> i32;
        pub fn IOServiceClose(conn: u32) -> i32;
        pub fn IOObjectRelease(object: u32) -> i32;
        pub fn IOConnectCallStructMethod(
            conn: u32,
            selector: u32,
            input: *const c_void,
            input_size: usize,
            output: *mut c_void,
            output_size: *mut usize,
        ) -> i32;
    }

    unsafe extern "C" {
        pub static mach_task_self_: u32;
    }
}

#[cfg(target_os = "macos")]
impl Smc {
    pub fn open() -> Result<Smc> {
        // SAFETY: plain IOKit calls; the matching dictionary is consumed by
        // IOServiceGetMatchingService, and the service released after use.
        unsafe {
            let matching = sys::IOServiceMatching(c"AppleSMC".as_ptr());
            if matching.is_null() {
                return Err(Error::Unsupported);
            }
            let service = sys::IOServiceGetMatchingService(0, matching);
            if service == 0 {
                return Err(Error::Unsupported);
            }
            let mut conn = 0;
            let code = sys::IOServiceOpen(service, sys::mach_task_self_, 0, &mut conn);
            sys::IOObjectRelease(service);
            if code != 0 {
                return Err(Error::Kernel {
                    call: "IOServiceOpen",
                    code,
                });
            }
            Ok(Smc { conn })
        }
    }

    fn call(&self, input: &sys::KeyData) -> Result<sys::KeyData> {
        let mut output = sys::KeyData::default();
        let mut size = std::mem::size_of::<sys::KeyData>();
        // SAFETY: both buffers are live `KeyData`s of the size passed.
        let code = unsafe {
            sys::IOConnectCallStructMethod(
                self.conn,
                sys::KERNEL_INDEX_SMC,
                (input as *const sys::KeyData).cast(),
                std::mem::size_of::<sys::KeyData>(),
                (&mut output as *mut sys::KeyData).cast(),
                &mut size,
            )
        };
        match code {
            0 => Ok(output),
            sys::IO_RETURN_NOT_PRIVILEGED => Err(Error::NotPrivileged),
            code => Err(Error::Kernel {
                call: "IOConnectCallStructMethod",
                code,
            }),
        }
    }

    fn check(key: Key, out: &sys::KeyData) -> Result<()> {
        match out.result {
            0 => Ok(()),
            sys::SMC_KEY_NOT_FOUND => Err(Error::NotFound(key)),
            code => Err(Error::Smc { key, code }),
        }
    }

    /// Reads `key` whose [`KeyInfo`] is already known, saving the key-info
    /// round trip [`SmcIo::read`] makes: for polling the same keys often.
    pub fn read_known(&self, key: Key, info: KeyInfo) -> Result<Value> {
        if info.size as usize > 32 {
            return Err(Error::BadValue(key));
        }
        let mut input = sys::KeyData {
            key: key.0,
            data8: sys::CMD_READ_BYTES,
            ..Default::default()
        };
        input.key_info.data_size = info.size;
        let out = self.call(&input)?;
        Self::check(key, &out)?;
        Ok(Value {
            key,
            info,
            bytes: out.bytes[..info.size as usize].to_vec(),
        })
    }

    /// How many keys the SMC has (`#KEY`).
    pub fn key_count(&self) -> Result<u32> {
        let v = self.read_f64(crate::KEY_COUNT)?;
        Ok(v as u32)
    }

    /// The key at `index`, in the SMC's own order (0..[`key_count`]).
    ///
    /// [`key_count`]: Smc::key_count
    pub fn key_at(&self, index: u32) -> Result<Key> {
        let input = sys::KeyData {
            data8: sys::CMD_READ_INDEX,
            data32: index,
            ..Default::default()
        };
        let out = self.call(&input)?;
        Self::check(Key(0), &out)?;
        Ok(Key(out.key))
    }

    /// Every key, skipping indexes the SMC won't name.
    pub fn keys(&self) -> Result<Vec<Key>> {
        let n = self.key_count()?;
        Ok((0..n).filter_map(|i| self.key_at(i).ok()).collect())
    }

    /// Every readable key starting with `T` that decodes as a number: the
    /// temperature sensors, in °C, under their model-specific names.
    pub fn temperatures(&self) -> Result<Vec<(Key, f64)>> {
        Ok(self
            .keys()?
            .into_iter()
            .filter(|k| k.bytes()[0] == b'T')
            .filter_map(|k| Some((k, self.read_f64(k).ok()?)))
            .collect())
    }
}

#[cfg(target_os = "macos")]
impl SmcIo for Smc {
    fn key_info(&self, key: Key) -> Result<KeyInfo> {
        let input = sys::KeyData {
            key: key.0,
            data8: sys::CMD_READ_KEYINFO,
            ..Default::default()
        };
        let out = self.call(&input)?;
        Self::check(key, &out)?;
        Ok(KeyInfo {
            size: out.key_info.data_size,
            data_type: DataType(out.key_info.data_type),
            attributes: out.key_info.attributes,
        })
    }

    fn read(&self, key: Key) -> Result<Value> {
        self.read_known(key, self.key_info(key)?)
    }

    fn write(&self, key: Key, bytes: &[u8]) -> Result<()> {
        let info = self.key_info(key)?;
        if bytes.len() != info.size as usize || bytes.len() > 32 {
            return Err(Error::BadValue(key));
        }
        let mut input = sys::KeyData {
            key: key.0,
            data8: sys::CMD_WRITE_BYTES,
            ..Default::default()
        };
        input.key_info.data_size = info.size;
        input.bytes[..bytes.len()].copy_from_slice(bytes);
        let out = self.call(&input)?;
        Self::check(key, &out)
    }
}

#[cfg(target_os = "macos")]
impl Drop for Smc {
    fn drop(&mut self) {
        // SAFETY: opened in `open` and not used after this.
        unsafe { sys::IOServiceClose(self.conn) };
    }
}

// The connection is a Mach port name, usable from any thread.
#[cfg(target_os = "macos")]
unsafe impl Send for Smc {}
#[cfg(target_os = "macos")]
unsafe impl Sync for Smc {}

#[cfg(not(target_os = "macos"))]
impl Smc {
    pub fn open() -> Result<Smc> {
        Err(Error::Unsupported)
    }
    pub fn key_count(&self) -> Result<u32> {
        Err(Error::Unsupported)
    }
    pub fn key_at(&self, _index: u32) -> Result<Key> {
        Err(Error::Unsupported)
    }
    pub fn keys(&self) -> Result<Vec<Key>> {
        Err(Error::Unsupported)
    }
    pub fn temperatures(&self) -> Result<Vec<(Key, f64)>> {
        Err(Error::Unsupported)
    }
    pub fn read_known(&self, _key: Key, _info: KeyInfo) -> Result<Value> {
        Err(Error::Unsupported)
    }
}

#[cfg(not(target_os = "macos"))]
impl SmcIo for Smc {
    fn key_info(&self, _key: Key) -> Result<KeyInfo> {
        Err(Error::Unsupported)
    }
    fn read(&self, _key: Key) -> Result<Value> {
        Err(Error::Unsupported)
    }
    fn write(&self, _key: Key, _bytes: &[u8]) -> Result<()> {
        Err(Error::Unsupported)
    }
}
