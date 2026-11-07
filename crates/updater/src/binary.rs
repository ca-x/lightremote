use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

use crate::{Error, Result, release::Target};

pub(crate) fn validate(file: &mut File, target: Target) -> Result<()> {
    file.seek(SeekFrom::Start(0))?;
    // Includes bounded PE header offsets; no candidate code or dynamic loader is run.
    let mut bytes = Vec::new();
    file.take(64 * 1024).read_to_end(&mut bytes)?;
    validate_header(&bytes, target)
}

fn validate_header(bytes: &[u8], target: Target) -> Result<()> {
    let u16_at = |offset| {
        bytes
            .get(offset..offset + 2)
            .and_then(|b| b.try_into().ok())
            .map(u16::from_le_bytes)
    };
    let u32_at = |offset| {
        bytes
            .get(offset..offset + 4)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let valid = match target {
        Target::LinuxX64 | Target::LinuxArm64 => {
            bytes.len() >= 64
                && bytes.starts_with(b"\x7fELF\x02\x01\x01")
                && matches!(u16_at(16), Some(2 | 3))
                && u16_at(18) == Some(if target == Target::LinuxX64 { 62 } else { 183 })
                && u32_at(20) == Some(1)
                && u16_at(52) == Some(64)
        }
        Target::MacX64 | Target::MacArm64 => {
            bytes.len() >= 32
                && u32_at(0) == Some(0xfeed_facf)
                && u32_at(4)
                    == Some(if target == Target::MacX64 {
                        0x0100_0007
                    } else {
                        0x0100_000c
                    })
                && u32_at(12) == Some(2)
        }
        Target::WindowsX64 => {
            if !bytes.starts_with(b"MZ") {
                return Err(Error::Binary);
            }
            let offset = u32_at(0x3c).ok_or(Error::Binary)? as usize;
            (64..=60 * 1024).contains(&offset)
                && bytes.get(offset..offset + 4) == Some(b"PE\0\0".as_slice())
                && u16_at(offset + 4) == Some(0x8664)
                && u16_at(offset + 20).is_some_and(|size| size >= 112)
                && u16_at(offset + 22)
                    .is_some_and(|flags| flags & 0x0002 != 0 && flags & 0x2000 == 0)
                && u16_at(offset + 24) == Some(0x020b)
                && bytes.len() >= offset + 24 + 112
        }
    };
    if valid { Ok(()) } else { Err(Error::Binary) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_scripts_wrong_machine_and_truncated_executables() {
        assert!(validate_header(b"#!/bin/sh\necho evil", Target::LinuxX64).is_err());
        let mut elf = vec![0; 64];
        elf[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        elf[16..18].copy_from_slice(&3_u16.to_le_bytes());
        elf[18..20].copy_from_slice(&62_u16.to_le_bytes());
        elf[20..24].copy_from_slice(&1_u32.to_le_bytes());
        elf[52..54].copy_from_slice(&64_u16.to_le_bytes());
        assert!(validate_header(&elf, Target::LinuxX64).is_ok());
        assert!(validate_header(&elf, Target::LinuxArm64).is_err());
        assert!(validate_header(&elf[..32], Target::LinuxX64).is_err());
        let mut mach = vec![0; 32];
        mach[..4].copy_from_slice(&0xfeed_facf_u32.to_le_bytes());
        mach[4..8].copy_from_slice(&0x0100_000c_u32.to_le_bytes());
        mach[12..16].copy_from_slice(&2_u32.to_le_bytes());
        assert!(validate_header(&mach, Target::MacArm64).is_ok());
        assert!(validate_header(&mach, Target::MacX64).is_err());
        let mut pe = vec![0; 512];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c..0x40].copy_from_slice(&64_u32.to_le_bytes());
        pe[64..68].copy_from_slice(b"PE\0\0");
        pe[68..70].copy_from_slice(&0x8664_u16.to_le_bytes());
        pe[84..86].copy_from_slice(&240_u16.to_le_bytes());
        pe[86..88].copy_from_slice(&2_u16.to_le_bytes());
        pe[88..90].copy_from_slice(&0x20b_u16.to_le_bytes());
        assert!(validate_header(&pe, Target::WindowsX64).is_ok());
        pe[68..70].copy_from_slice(&0xaa64_u16.to_le_bytes());
        assert!(validate_header(&pe, Target::WindowsX64).is_err());
    }
}
