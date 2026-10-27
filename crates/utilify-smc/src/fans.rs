//! Fan keys: `FNum` fans, each with `F{n}Ac` (current rpm), `F{n}Mn`/`F{n}Mx`
//! (range), `F{n}Tg` (target) and, on Apple Silicon, `F{n}Md` (mode: 0
//! automatic, 1 manual, 3 stopped by the system, as M4 Macs do at idle). Speeds are `flt ` on Apple Silicon and `fpe2` on
//! Intel; [`SmcIo::read_f64`] decodes either.

use crate::{Error, FAN_COUNT, FAN_FORCE_MASK, Key, Result, SmcIo};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FanInfo {
    pub index: u8,
    /// Current speed, rpm.
    pub current: f32,
    pub min: f32,
    pub max: f32,
    /// The speed the SMC is steering towards; the current speed where the
    /// SMC has no target key.
    pub target: f32,
    /// Held at a fixed speed by some tool rather than the system.
    pub manual: bool,
}

/// `F{index}{suffix}`, like `F0Ac`. Fan keys only have room for one digit.
pub fn fan_key(index: u8, suffix: &str) -> Option<Key> {
    if index > 9 || suffix.len() != 2 {
        return None;
    }
    Key::new(&format!("F{index}{suffix}"))
}

pub fn fan_count(io: &impl SmcIo) -> Result<u8> {
    let n = io.read_f64(FAN_COUNT)?;
    Ok(n.clamp(0.0, 10.0) as u8)
}

pub fn read_fan(io: &impl SmcIo, index: u8) -> Result<FanInfo> {
    let key = |s| fan_key(index, s).ok_or(Error::Unsupported);
    let read = |s| io.read_f64(key(s)?).map(|v| v as f32);
    let current = read("Ac")?;
    let manual = match fan_key(index, "Md").map(|k| io.read_f64(k)) {
        // Only 1 is manual: idle M4 Macs park their fans at mode 3.
        Some(Ok(mode)) => mode == 1.0,
        // Intel: bit n of the forced-fans mask.
        _ => io
            .read_f64(FAN_FORCE_MASK)
            .is_ok_and(|m| (m as u32) >> index & 1 == 1),
    };
    Ok(FanInfo {
        index,
        current,
        min: read("Mn")?,
        max: read("Mx")?,
        target: read("Tg").unwrap_or(current),
        manual,
    })
}

/// Every fan that reads; empty on fanless Macs.
pub fn read_fans(io: &impl SmcIo) -> Result<Vec<FanInfo>> {
    let n = match fan_count(io) {
        Ok(n) => n,
        Err(Error::NotFound(_)) => 0,
        Err(e) => return Err(e),
    };
    Ok((0..n).filter_map(|i| read_fan(io, i).ok()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DataType;
    use crate::fake::FakeSmc;

    #[test]
    fn reads_apple_silicon_fans() {
        let smc = FakeSmc::apple_silicon(&[(1000.0, 4900.0), (1200.0, 5700.0)]);
        smc.set_f64(fan_key(1, "Md").unwrap(), 1.0);
        let fans = read_fans(&smc).unwrap();
        assert_eq!(fans.len(), 2);
        assert_eq!(fans[0].min, 1000.0);
        assert_eq!(fans[1].max, 5700.0);
        assert!(!fans[0].manual);
        assert!(fans[1].manual);
    }

    #[test]
    fn stopped_fans_are_not_manual() {
        let smc = FakeSmc::apple_silicon(&[(1350.0, 5777.0)]);
        smc.set_f64(fan_key(0, "Md").unwrap(), 3.0);
        assert!(!read_fans(&smc).unwrap()[0].manual);
    }

    #[test]
    fn reads_intel_fans() {
        let smc = FakeSmc::new();
        smc.insert(FAN_COUNT, DataType::UI8, &[1]);
        smc.insert(fan_key(0, "Ac").unwrap(), DataType::FPE2, &[0x17, 0x70]);
        smc.insert(fan_key(0, "Mn").unwrap(), DataType::FPE2, &[0x17, 0x70]);
        smc.insert(fan_key(0, "Mx").unwrap(), DataType::FPE2, &[0x5d, 0xc0]);
        smc.insert(FAN_FORCE_MASK, DataType::UI16, &[0, 1]);
        let fans = read_fans(&smc).unwrap();
        assert_eq!(fans.len(), 1);
        assert_eq!(fans[0].current, 1500.0);
        assert_eq!(fans[0].max, 6000.0);
        assert_eq!(fans[0].target, 1500.0);
        assert!(fans[0].manual);
    }

    #[test]
    fn fanless_is_empty() {
        assert_eq!(read_fans(&FakeSmc::new()).unwrap(), vec![]);
    }

    #[test]
    fn fan_keys_are_one_digit() {
        assert_eq!(fan_key(3, "Tg").unwrap().to_string(), "F3Tg");
        assert_eq!(fan_key(10, "Tg"), None);
        assert_eq!(fan_key(0, "T"), None);
    }
}
