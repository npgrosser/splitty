//! Identity observations never signal/kill a saved PID. Missing authority or
//! process information is Unknown; only the same verified OS view proves exit.
use super::{Identity, Scope};
use std::{io, path::PathBuf};

pub(super) enum Policy {
    Native {
        scope: Scope,
        directory: PathBuf,
    },
    /// Windows/WSL have reachable localhost listeners across runtimes without
    /// a shared reservation broker. All reverse-share entry points refuse it.
    #[cfg(not(target_os = "macos"))]
    NoSharing,
}

pub(super) enum Observed {
    Live,
    Terminated,
    Unknown,
}

fn unsupported(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, message)
}

#[cfg(unix)]
fn state_directory() -> io::Result<PathBuf> {
    // HOME, XDG and config overrides may deliberately name separate workspaces.
    // Reservation authority is the OS account's persistent home, not those.
    let uid = nix::unistd::geteuid();
    let user = nix::unistd::User::from_uid(uid)?
        .ok_or_else(|| unsupported("no system account home for forwarding reservations"))?;
    if user.uid != uid || !user.dir.is_absolute() {
        return Err(unsupported(
            "invalid system account home for forwarding reservations",
        ));
    }
    #[cfg(target_os = "macos")]
    let directory = user
        .dir
        .join("Library/Application Support/Splitty/exposure");
    #[cfg(not(target_os = "macos"))]
    let directory = user.dir.join(".local/state/splitty/exposure");
    Ok(directory)
}

#[cfg(target_os = "linux")]
fn small_file(path: &str, limit: u64) -> io::Result<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > limit {
        return Err(unsupported("OS identity exceeds its bound"));
    }
    Ok(text)
}

#[cfg(target_os = "linux")]
fn namespace(name: &str) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::metadata(format!("/proc/self/ns/{name}"))?.ino())
}

#[cfg(target_os = "linux")]
fn root_mount(path: &str) -> io::Result<String> {
    let text = small_file(path, 1024 * 1024)?;
    let mut root = None;
    for line in text.lines() {
        if line.split_whitespace().nth(4) == Some("/") {
            if root.is_some() || line.len() > 16 * 1024 {
                return Err(unsupported("ambiguous initial root mount authority"));
            }
            root = Some(line.to_owned());
        }
    }
    root.ok_or_else(|| unsupported("cannot verify the initial mount authority"))
}

#[cfg(target_os = "linux")]
pub(super) fn policy() -> io::Result<Policy> {
    let release = small_file("/proc/sys/kernel/osrelease", 1024)?;
    if release.to_ascii_lowercase().contains("microsoft") {
        return Ok(Policy::NoSharing);
    }
    // Published Linux UAPI init_ns_ino values. Unknown versions/views fail
    // closed rather than infer global PID visibility from a missing process.
    if namespace("pid")? != 0xefff_fffc || namespace("user")? != 0xefff_fffd {
        return Err(unsupported(
            "forwarding reservations require the initial Linux PID/user namespaces",
        ));
    }
    let budget = splitty_transport::budget::ConnectionBudget::default();
    let _identity_bytes = budget
        .reserve(3 * 1024 * 1024, false)
        .map_err(io::Error::other)?;
    let mount = root_mount("/proc/self/mountinfo")?;
    if mount != root_mount("/proc/1/mountinfo")? {
        return Err(unsupported(
            "forwarding reservations cannot coordinate a private mount/root namespace",
        ));
    }
    let machine = small_file("/etc/machine-id", 128)?.trim().to_owned();
    let boot = small_file("/proc/sys/kernel/random/boot_id", 128)?
        .trim()
        .to_owned();
    if machine.len() != 32
        || !machine.bytes().all(|byte| byte.is_ascii_hexdigit())
        || boot.len() != 36
        || !boot
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err(unsupported(
            "cannot verify machine/boot identity for forwarding reservations",
        ));
    }
    Ok(Policy::Native {
        directory: state_directory()?,
        scope: Scope {
            machine,
            boot,
            network: namespace("net")?,
            pid_namespace: namespace("pid")?,
            uid: nix::unistd::geteuid().as_raw(),
        },
    })
}

#[cfg(target_os = "linux")]
pub(super) fn identity(pid: u32) -> io::Result<Identity> {
    use std::os::unix::fs::MetadataExt;
    let path = format!("/proc/{pid}/stat");
    let text = small_file(&path, 8192)?;
    let owner = std::fs::metadata(format!("/proc/{pid}"))?.uid();
    let (_, rest) = text
        .rsplit_once(") ")
        .ok_or_else(|| unsupported("invalid Linux process identity"))?;
    let fields: Vec<_> = rest.split_whitespace().collect();
    let start = fields
        .get(19)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| unsupported("missing Linux process birth identity"))?;
    let state = *fields
        .first()
        .ok_or_else(|| unsupported("missing Linux process state"))?;
    if matches!(state, "Z" | "X" | "x") {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    }
    Ok(Identity {
        pid,
        start,
        subsecond: 0,
        uid: owner,
    })
}

#[cfg(target_os = "macos")]
pub(super) fn policy() -> io::Result<Policy> {
    use nix::libc;
    let mut roots = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::uninit();
    let root_size = i32::try_from(std::mem::size_of::<libc::proc_vnodepathinfo>())
        .expect("fixed vnode info size");
    let own_pid =
        i32::try_from(std::process::id()).map_err(|_| unsupported("invalid macOS process id"))?;
    // SAFETY: this flavor fills precisely proc_vnodepathinfo. XNU zeroes the
    // output and fills pvi_rdir only when the process has an alternate/chroot
    // root vnode (fd_rdir != NULL). Querying self needs no PID1/ptrace grant.
    let result = unsafe {
        libc::proc_pidinfo(
            own_pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            roots.as_mut_ptr().cast(),
            root_size,
        )
    };
    if result != root_size {
        return Err(unsupported("cannot verify the global macOS root authority"));
    }
    // SAFETY: proc_pidinfo returned the full initialized structure.
    let roots = unsafe { roots.assume_init() };
    let root = roots.pvi_rdir;
    if root.vip_vi.vi_type != 0
        || root.vip_vi.vi_stat.vst_ino != 0
        || root.vip_vi.vi_stat.vst_dev != 0
        || root.vip_path[0][0] != 0
    {
        return Err(unsupported(
            "forwarding reservations cannot coordinate a private macOS root",
        ));
    }
    let mut machine = [0u8; 16];
    let timeout = libc::timespec {
        tv_sec: 1,
        tv_nsec: 0,
    };
    // SAFETY: the buffers and sizes match the OS declarations; short/failed
    // identity responses never authorize a reservation or process cleanup.
    if unsafe { libc::gethostuuid(machine.as_mut_ptr(), &timeout) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // Wall-clock corrections can change KERN_BOOTTIME while workers remain
    // alive. Only the kernel's boot-session UUID authorizes a boot change.
    let mut boot = [0u8; 128];
    let mut length = boot.len();
    // SAFETY: the name is NUL terminated, output is a fixed byte array and no
    // replacement value is supplied. Failed/short/unrecognized data refuses.
    if unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            boot.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || length == 0
        || length > boot.len()
    {
        return Err(unsupported("cannot verify macOS boot-session identity"));
    }
    let boot = std::ffi::CStr::from_bytes_with_nul(&boot[..length])
        .ok()
        .and_then(|boot| boot.to_str().ok())
        .filter(|boot| {
            boot.len() == 36
                && boot
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        })
        .ok_or_else(|| unsupported("invalid macOS boot-session identity"))?
        .to_ascii_lowercase();
    Ok(Policy::Native {
        directory: state_directory()?,
        scope: Scope {
            machine: machine.iter().map(|byte| format!("{byte:02x}")).collect(),
            boot,
            network: 0,
            pid_namespace: 0,
            uid: nix::unistd::geteuid().as_raw(),
        },
    })
}

#[cfg(target_os = "macos")]
pub(super) fn identity(pid: u32) -> io::Result<Identity> {
    use nix::libc;
    let pid_arg = i32::try_from(pid).map_err(|_| unsupported("invalid macOS process id"))?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size =
        i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).expect("fixed process info size");
    // SAFETY: the BSD process-info flavor uses exactly this output structure.
    let result = unsafe {
        libc::proc_pidinfo(
            pid_arg,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if result != size {
        // A failed/short identity query alone is not termination proof.
        // SAFETY: signal zero only queries a PID; it delivers no signal.
        if unsafe { libc::kill(pid_arg, 0) } == -1
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        return Err(unsupported("macOS process identity is unavailable"));
    }
    // SAFETY: proc_pidinfo returned the full initialized output size.
    let info = unsafe { info.assume_init() };
    if info.pbi_pid != pid {
        return Err(unsupported("macOS process identity mismatch"));
    }
    Ok(Identity {
        pid,
        start: info.pbi_start_tvsec,
        subsecond: info.pbi_start_tvusec,
        uid: info.pbi_uid,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) fn policy() -> io::Result<Policy> {
    Ok(Policy::NoSharing)
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) fn identity(_pid: u32) -> io::Result<Identity> {
    Err(unsupported("forwarding worker identity is unsupported"))
}

pub(super) fn observe(scope: &Scope, expected: &Identity) -> Observed {
    // The registry calls this only after matching its machine, boot and
    // observer PID namespace. Permission errors and reused PIDs remain unknown.
    if expected.uid != scope.uid {
        return Observed::Unknown;
    }
    match identity(expected.pid) {
        Ok(current) if &current == expected => Observed::Live,
        Ok(_) => Observed::Unknown,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Observed::Terminated,
        Err(_) => Observed::Unknown,
    }
}
