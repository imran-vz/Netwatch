//! An in-memory SMC for tests: keys with types and values, a log of every
//! write, and switches to make writes fail or not stick.

use crate::{DataType, Error, FAN_COUNT, FAN_FORCE, Key, KeyInfo, Result, SmcIo, Value, fan_key};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct State {
    keys: BTreeMap<Key, Value>,
    writes: Vec<(Key, Vec<u8>)>,
    deny_writes: bool,
    ignored: BTreeSet<Key>,
    ignore_next: BTreeMap<Key, u32>,
    failures: BTreeMap<Key, u32>,
}

#[derive(Default)]
pub struct FakeSmc {
    state: RefCell<State>,
}

impl FakeSmc {
    pub fn new() -> FakeSmc {
        FakeSmc::default()
    }

    /// An Apple Silicon SMC with these fans' (min, max), all automatic and
    /// idling at min, and `Ftst` at 0.
    pub fn apple_silicon(fans: &[(f32, f32)]) -> FakeSmc {
        let smc = FakeSmc::new();
        smc.insert(FAN_COUNT, DataType::UI8, &[fans.len() as u8]);
        smc.insert(FAN_FORCE, DataType::UI8, &[0]);
        for (i, &(min, max)) in fans.iter().enumerate() {
            let flt = |suffix, v: f32| {
                smc.insert(
                    fan_key(i as u8, suffix).unwrap(),
                    DataType::FLT,
                    &v.to_le_bytes(),
                )
            };
            flt("Ac", min);
            flt("Mn", min);
            flt("Mx", max);
            flt("Tg", min);
            smc.insert(fan_key(i as u8, "Md").unwrap(), DataType::UI8, &[0]);
        }
        smc
    }

    pub fn insert(&self, key: Key, data_type: DataType, bytes: &[u8]) {
        let info = KeyInfo {
            size: bytes.len() as u32,
            data_type,
            attributes: 0,
        };
        self.state.borrow_mut().keys.insert(
            key,
            Value {
                key,
                info,
                bytes: bytes.to_vec(),
            },
        );
    }

    /// Sets an existing key from a number, bypassing the write log.
    pub fn set_f64(&self, key: Key, v: f64) {
        let mut st = self.state.borrow_mut();
        let value = st.keys.get_mut(&key).expect("no such fake key");
        value.bytes = crate::encode(value.info, v).expect("value doesn't fit");
    }

    pub fn get_f64(&self, key: Key) -> Option<f64> {
        self.state.borrow().keys.get(&key)?.as_f64()
    }

    /// Every write so far, in order, including ones that failed.
    pub fn writes(&self) -> Vec<(Key, Vec<u8>)> {
        self.state.borrow().writes.clone()
    }

    pub fn clear_writes(&self) {
        self.state.borrow_mut().writes.clear();
    }

    /// Make every write fail as it does without root.
    pub fn deny_writes(&self, deny: bool) {
        self.state.borrow_mut().deny_writes = deny;
    }

    /// Accept writes to `key` without changing it, like an SMC that ignores
    /// a manual-mode request.
    pub fn ignore_writes(&self, key: Key) {
        self.state.borrow_mut().ignored.insert(key);
    }

    /// Accept the next `n` writes to `key` without changing it, like Apple
    /// Silicon before its thermal daemon lets go of the fans.
    pub fn ignore_next_writes(&self, key: Key, n: u32) {
        self.state.borrow_mut().ignore_next.insert(key, n);
    }

    /// Fail the next `n` writes to `key`.
    pub fn fail_writes(&self, key: Key, n: u32) {
        self.state.borrow_mut().failures.insert(key, n);
    }
}

impl SmcIo for FakeSmc {
    fn key_info(&self, key: Key) -> Result<KeyInfo> {
        let st = self.state.borrow();
        st.keys
            .get(&key)
            .map(|v| v.info)
            .ok_or(Error::NotFound(key))
    }

    fn read(&self, key: Key) -> Result<Value> {
        self.state
            .borrow()
            .keys
            .get(&key)
            .cloned()
            .ok_or(Error::NotFound(key))
    }

    fn write(&self, key: Key, bytes: &[u8]) -> Result<()> {
        let mut st = self.state.borrow_mut();
        st.writes.push((key, bytes.to_vec()));
        if st.deny_writes {
            return Err(Error::NotPrivileged);
        }
        if let Some(n) = st.failures.get_mut(&key).filter(|n| **n > 0) {
            *n -= 1;
            return Err(Error::Smc { key, code: 0x82 });
        }
        let mut ignored = st.ignored.contains(&key);
        if let Some(n) = st.ignore_next.get_mut(&key).filter(|n| **n > 0) {
            *n -= 1;
            ignored = true;
        }
        let value = st.keys.get_mut(&key).ok_or(Error::NotFound(key))?;
        if bytes.len() != value.info.size as usize {
            return Err(Error::BadValue(key));
        }
        if !ignored {
            value.bytes = bytes.to_vec();
        }
        Ok(())
    }
}
