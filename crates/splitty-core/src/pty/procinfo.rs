//! Platform-specific process info lookups (name, cwd, cmdline, RSS).
//!
//! Uses `/proc` on Linux and `ps`/`lsof` on macOS. Returns `None` on
//! unsupported platforms or if the process cannot be inspected.

/// Information about a running process.
#[derive(Clone, Debug, Default)]
pub struct ProcessInfo {
    pub name: String,
    pub cmdline: String,
    pub cwd: String,
}

/// Checks whether a cmdline plausibly belongs to the process identified by comm.
///
/// Compares against the basename of argv\[0\] (not the full cmdline string) to
/// avoid false positives from directory names. Also handles comm modified via
/// prctl(PR_SET_NAME) (e.g. Node.js threads set comm to "node-MainThread")
/// by checking the prefix before the hyphen.
pub fn cmdline_matches_comm(cmdline: &str, comm: &str) -> bool {
    let argv0 = match cmdline.find(' ') {
        Some(i) => &cmdline[..i],
        None => cmdline,
    };
    let base = match argv0.rfind(['/', '\\']) {
        Some(i) => &argv0[i + 1..],
        None => argv0,
    };

    // Direct match: basename of argv[0] contains comm or vice versa.
    if base == comm || base.contains(comm) || comm.contains(base) {
        return true;
    }

    // Thread-name suffix: comm="node-MainThread" -> check "node" against base.
    if let Some(i) = comm.find('-') {
        let prefix = &comm[..i];
        if base == prefix || base.contains(prefix) {
            return true;
        }
    }

    false
}

/// Read process info (name, cwd, cmdline) for the given PID.
///
/// Platform-specific: uses `ps`/`lsof` on macOS and `/proc` on Linux.
#[cfg(target_os = "macos")]
pub fn process_info(pid: u32) -> Option<ProcessInfo> {
    use std::process::Command;

    let pid_str = pid.to_string();

    let out = Command::new("ps")
        .args(["-o", "comm=", "-p", &pid_str])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() {
        return None;
    }

    let cmdline = match Command::new("ps")
        .args(["-o", "args=", "-p", &pid_str])
        .output()
    {
        Ok(cmd_out) if cmd_out.status.success() => {
            let raw = String::from_utf8_lossy(&cmd_out.stdout).trim().to_string();
            if cmdline_matches_comm(&raw, &name) {
                raw
            } else {
                name.clone()
            }
        }
        _ => name.clone(),
    };

    let cwd = match Command::new("lsof")
        .args(["-a", "-p", &pid_str, "-d", "cwd", "-Fn"])
        .output()
    {
        Ok(lsof_out) if lsof_out.status.success() => {
            let text = String::from_utf8_lossy(&lsof_out.stdout);
            text.lines()
                .find(|line| line.starts_with('n'))
                .map(|line| line[1..].to_string())
                .unwrap_or_default()
        }
        _ => String::new(),
    };

    Some(ProcessInfo { name, cmdline, cwd })
}

#[cfg(target_os = "linux")]
pub fn process_info(pid: u32) -> Option<ProcessInfo> {
    use std::fs;

    let comm_data = fs::read_to_string(format!("/proc/{}/comm", pid)).ok()?;
    let name = comm_data.trim().to_string();

    let cmdline = match fs::read(format!("/proc/{}/cmdline", pid)) {
        Ok(data) if !data.is_empty() => {
            let raw = String::from_utf8_lossy(&data)
                .replace('\0', " ")
                .trim_end()
                .to_string();
            if cmdline_matches_comm(&raw, &name) {
                raw
            } else {
                name.clone()
            }
        }
        _ => name.clone(),
    };

    let cwd = fs::read_link(format!("/proc/{}/cwd", pid))
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_default();

    Some(ProcessInfo { name, cmdline, cwd })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn process_info(_pid: u32) -> Option<ProcessInfo> {
    None
}

/// Read the resident set size (in bytes) of a process.
#[cfg(target_os = "linux")]
pub fn read_rss(pid: u32) -> i64 {
    let data = match std::fs::read_to_string(format!("/proc/{}/statm", pid)) {
        Ok(d) => d,
        Err(_) => return 0,
    };
    let fields: Vec<&str> = data.split_whitespace().collect();
    if fields.len() < 2 {
        return 0;
    }
    match fields[1].parse::<i64>() {
        Ok(pages) => pages * 4096,
        Err(_) => 0,
    }
}

#[cfg(not(target_os = "linux"))]
pub fn read_rss(_pid: u32) -> i64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cmdline_matches_comm_direct() {
        assert!(cmdline_matches_comm("vim", "vim"));
    }

    #[test]
    fn test_cmdline_matches_comm_path() {
        assert!(cmdline_matches_comm("/usr/bin/vim", "vim"));
    }

    #[test]
    fn test_cmdline_matches_comm_thread_name() {
        assert!(cmdline_matches_comm("node", "node-MainThread"));
    }

    #[test]
    fn test_cmdline_matches_comm_no_match() {
        assert!(!cmdline_matches_comm("vim", "emacs"));
    }
}
