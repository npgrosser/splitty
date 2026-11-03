//! Darwin's extended ACLs are independent of Unix mode bits. Inspect the
//! actual FD's bounded extended-security attribute; do not parse ls output or
//! interpret an ACL retrieval failure as an empty ACL.
use super::denied;
use std::io;

#[cfg(target_os = "macos")]
pub(super) fn check_file(file: &std::fs::File, private: bool) -> io::Result<()> {
    use nix::libc;
    use std::os::fd::AsRawFd;
    let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fstatfs receives its complete correctly sized output structure.
    if unsafe { libc::fstatfs(file.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatfs successfully initialized the complete output.
    let filesystem = unsafe { filesystem.assume_init() };
    if filesystem.f_flags & u32::try_from(libc::MNT_IGNORE_OWNERSHIP).expect("mount flag") != 0 {
        return Err(denied(
            "forwarding reservations require enforced filesystem ownership",
        ));
    }
    let mut attributes = libc::attrlist {
        bitmapcount: 5,
        reserved: 0,
        commonattr: libc::ATTR_CMN_EXTENDED_SECURITY,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut bytes = [0; 8192];
    // SAFETY: the attribute list requests just one attrreference plus its
    // bounded kauth_filesec. Oversized/unsupported output fails admission.
    let result = unsafe {
        libc::fgetattrlist(
            file.as_raw_fd(),
            (&mut attributes as *mut libc::attrlist).cast(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            0,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    validate(&bytes, private)
}

fn number(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let field: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| denied("short macOS security attribute"))?
        .try_into()
        .expect("four-byte slice");
    Ok(u32::from_ne_bytes(field))
}
fn validate(bytes: &[u8], private: bool) -> io::Result<()> {
    let total =
        usize::try_from(number(bytes, 0)?).map_err(|_| denied("invalid macOS security size"))?;
    if total < 12 || total > bytes.len() {
        return Err(denied("invalid macOS security attribute size"));
    }
    let bytes = &bytes[..total];
    let length =
        usize::try_from(number(bytes, 8)?).map_err(|_| denied("invalid macOS ACL size"))?;
    if length == 0 {
        return if total == 12 {
            Ok(())
        } else {
            Err(denied("unexpected data after an empty macOS ACL"))
        };
    }
    let offset = i32::from_ne_bytes(bytes[4..8].try_into().expect("reference offset"));
    let start = usize::try_from(offset)
        .ok()
        .filter(|offset| *offset >= 8)
        .and_then(|offset| offset.checked_add(4))
        .ok_or_else(|| denied("invalid macOS ACL reference"))?;
    let end = start
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| denied("macOS ACL exceeds attribute"))?;
    let acl = &bytes[start..end];
    if acl.len() < 44 || number(acl, 0)? != 0x012c_c16d {
        return Err(denied("invalid macOS security descriptor"));
    }
    let count = number(acl, 36)?;
    if count == u32::MAX {
        return if acl.len() == 44 {
            Ok(())
        } else {
            Err(denied("invalid empty macOS ACL"))
        };
    }
    let count = usize::try_from(count).map_err(|_| denied("invalid macOS ACL entry count"))?;
    if count > 128 || acl.len() != 44 + count * 24 {
        return Err(denied("invalid macOS ACL entry count"));
    }
    // Read/search rights on ancestors cannot replace the authority. Private
    // registry objects conservatively accept no positive ACL access grant.
    const READ_ONLY: u32 =
        (1 << 1) | (1 << 3) | (1 << 7) | (1 << 9) | (1 << 11) | (1 << 20) | (1 << 22) | (1 << 24);
    const KNOWN_FLAGS: u32 = 0xf | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7) | (1 << 8);
    for entry in acl[44..].as_chunks::<24>().0 {
        let flags = number(entry, 16)?;
        let rights = number(entry, 20)?;
        if flags & !KNOWN_FLAGS != 0 {
            return Err(denied("unknown macOS ACL entry flags"));
        }
        match flags & 0xf {
            2 => {} // Deny ACEs cannot add authority (including default home deny-delete).
            1 if (!private && rights & !READ_ONLY == 0) || rights == 0 => {}
            1 => {
                return Err(denied(
                    "macOS ACL grants access to the forwarding reservation authority",
                ))
            }
            _ => return Err(denied("unknown macOS ACL entry kind")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn attribute(entries: &[(u32, u32)]) -> Vec<u8> {
        let mut bytes = vec![0; 12 + 44 + entries.len() * 24];
        let total = u32::try_from(bytes.len()).unwrap();
        bytes[..4].copy_from_slice(&total.to_ne_bytes());
        bytes[4..8].copy_from_slice(&8i32.to_ne_bytes());
        bytes[8..12].copy_from_slice(&(total - 12).to_ne_bytes());
        bytes[12..16].copy_from_slice(&0x012c_c16du32.to_ne_bytes());
        bytes[48..52].copy_from_slice(&u32::try_from(entries.len()).unwrap().to_ne_bytes());
        for (index, &(flags, rights)) in entries.iter().enumerate() {
            let start = 12 + 44 + index * 24;
            bytes[start + 16..start + 20].copy_from_slice(&flags.to_ne_bytes());
            bytes[start + 20..start + 24].copy_from_slice(&rights.to_ne_bytes());
        }
        bytes
    }
    #[test]
    fn inherited_grants_cannot_hide_behind_private_modes_and_default_denials_are_safe() {
        let deny_delete = attribute(&[(2, 1 << 4)]);
        assert!(validate(&deny_delete, true).is_ok());
        let read = attribute(&[(1, 1 << 1)]);
        assert!(validate(&read, false).is_ok());
        assert!(validate(&read, true).is_err());
        for right in [
            1 << 2,
            1 << 4,
            1 << 5,
            1 << 6,
            1 << 8,
            1 << 10,
            1 << 12,
            1 << 13,
            1 << 21,
            1 << 23,
            1 << 31,
        ] {
            for inherited in [0, (1 << 4) | (1 << 5) | (1 << 6), 1 << 8] {
                let writable = attribute(&[(1 | inherited, right)]);
                assert!(validate(&writable, false).is_err());
                assert!(validate(&writable, true).is_err());
            }
        }
    }
    #[test]
    fn incomplete_offsets_lengths_kinds_and_security_descriptors_are_not_empty_acls() {
        let valid = attribute(&[(2, 1 << 4)]);
        for length in 0..valid.len() {
            assert!(validate(&valid[..length], true).is_err());
        }
        for (offset, value) in [
            (4, u32::MAX),
            (4, 0),
            (8, u32::MAX),
            (12, 0),
            (48, 129),
            (72, 3),
            (72, 1 << 31),
        ] {
            let mut invalid = valid.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
            assert!(validate(&invalid, true).is_err(), "{offset}/{value}");
        }
        let mut empty = [0; 12];
        empty[..4].copy_from_slice(&12u32.to_ne_bytes());
        assert!(validate(&empty, true).is_ok());
    }
}
