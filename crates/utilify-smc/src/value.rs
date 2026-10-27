//! Numeric SMC value types. Integers and fixed point are big-endian; `flt `
//! is a little-endian f32 (Apple Silicon reports most values that way).

use crate::{DataType, KeyInfo};

/// `bytes` as a number, if `ty` is a numeric type and the size fits.
pub fn decode(ty: DataType, bytes: &[u8]) -> Option<f64> {
    let be2 = || bytes.get(..2).map(|b| [b[0], b[1]]);
    Some(match ty {
        DataType::FLT => {
            let v = f32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
            if !v.is_finite() {
                return None;
            }
            v as f64
        }
        DataType::UI8 | DataType::FLAG => *bytes.first()? as f64,
        DataType::SI8 => *bytes.first()? as i8 as f64,
        DataType::UI16 => u16::from_be_bytes(be2()?) as f64,
        DataType::SI16 => i16::from_be_bytes(be2()?) as f64,
        DataType::UI32 => u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?) as f64,
        DataType::SP78 => i16::from_be_bytes(be2()?) as f64 / 256.0,
        DataType::FPE2 => u16::from_be_bytes(be2()?) as f64 / 4.0,
        _ => return None,
    })
}

/// `v` encoded as the key's type, or `None` if the type isn't numeric or
/// `v` doesn't fit it. Integer types round to the nearest value.
pub fn encode(info: KeyInfo, v: f64) -> Option<Vec<u8>> {
    if !v.is_finite() {
        return None;
    }
    let int = |lo: f64, hi: f64| {
        let r = v.round();
        (lo..=hi).contains(&r).then_some(r)
    };
    let out = match info.data_type {
        DataType::FLT => (v as f32).to_le_bytes().to_vec(),
        DataType::UI8 | DataType::FLAG => vec![int(0.0, u8::MAX as f64)? as u8],
        DataType::SI8 => vec![int(i8::MIN as f64, i8::MAX as f64)? as i8 as u8],
        DataType::UI16 => (int(0.0, u16::MAX as f64)? as u16).to_be_bytes().to_vec(),
        DataType::SI16 => (int(i16::MIN as f64, i16::MAX as f64)? as i16)
            .to_be_bytes()
            .to_vec(),
        DataType::UI32 => (int(0.0, u32::MAX as f64)? as u32).to_be_bytes().to_vec(),
        DataType::SP78 => {
            let r = (v * 256.0).round();
            (i16::MIN as f64..=i16::MAX as f64)
                .contains(&r)
                .then_some(())?;
            (r as i16).to_be_bytes().to_vec()
        }
        DataType::FPE2 => {
            let r = (v * 4.0).round();
            (0.0..=u16::MAX as f64).contains(&r).then_some(())?;
            (r as u16).to_be_bytes().to_vec()
        }
        _ => return None,
    };
    // The SMC wants exactly the key's size.
    if out.len() != info.size as usize {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(data_type: DataType, size: u32) -> KeyInfo {
        KeyInfo {
            size,
            data_type,
            attributes: 0,
        }
    }

    #[test]
    fn decodes_each_type() {
        assert_eq!(
            decode(DataType::FLT, &5777.0f32.to_le_bytes()),
            Some(5777.0)
        );
        assert_eq!(decode(DataType::UI8, &[2]), Some(2.0));
        assert_eq!(decode(DataType::SI8, &[0xff]), Some(-1.0));
        assert_eq!(decode(DataType::UI16, &[0x01, 0x00]), Some(256.0));
        assert_eq!(decode(DataType::SI16, &[0xff, 0xfe]), Some(-2.0));
        assert_eq!(decode(DataType::UI32, &[0, 0, 0x0d, 0x86]), Some(3462.0));
        // 0x3280 = 50.5 degrees.
        assert_eq!(decode(DataType::SP78, &[0x32, 0x80]), Some(50.5));
        assert_eq!(decode(DataType::SP78, &[0xff, 0x00]), Some(-1.0));
        // 0x5a41 >> 2 = 5776.25 rpm.
        assert_eq!(decode(DataType::FPE2, &[0x5a, 0x41]), Some(5776.25));
    }

    #[test]
    fn rejects_short_unknown_and_non_finite() {
        assert_eq!(decode(DataType::FLT, &[0, 0]), None);
        assert_eq!(decode(DataType::UI8, &[]), None);
        assert_eq!(
            decode(DataType(u32::from_be_bytes(*b"ch8*")), b"abcd"),
            None
        );
        assert_eq!(decode(DataType::FLT, &f32::NAN.to_le_bytes()), None);
    }

    #[test]
    fn encodes_and_round_trips() {
        assert_eq!(encode(info(DataType::UI8, 1), 1.0), Some(vec![1]));
        assert_eq!(
            encode(info(DataType::FLT, 4), 2500.0),
            Some(2500.0f32.to_le_bytes().to_vec())
        );
        assert_eq!(
            encode(info(DataType::FPE2, 2), 5776.25),
            Some(vec![0x5a, 0x41])
        );
        assert_eq!(
            encode(info(DataType::SP78, 2), 50.5),
            Some(vec![0x32, 0x80])
        );
        for (ty, size, v) in [
            (DataType::UI16, 2, 1234.0),
            (DataType::UI32, 4, 70_000.0),
            (DataType::SI16, 2, -300.0),
        ] {
            let b = encode(info(ty, size), v).unwrap();
            assert_eq!(decode(ty, &b), Some(v));
        }
    }

    #[test]
    fn encode_refuses_out_of_range_and_wrong_size() {
        assert_eq!(encode(info(DataType::UI8, 1), 256.0), None);
        assert_eq!(encode(info(DataType::UI8, 1), -1.0), None);
        assert_eq!(encode(info(DataType::FLT, 4), f64::INFINITY), None);
        assert_eq!(encode(info(DataType::FLT, 2), 1.0), None);
        assert_eq!(encode(info(DataType::FPE2, 2), -4.0), None);
    }
}
