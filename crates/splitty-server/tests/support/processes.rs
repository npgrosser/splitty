//! Linux smoke commands run in their own process group. Reparenting does not
//! remove SSH/proxy descendants from that group, so leader exit alone cannot
//! make the cleanup assertion pass. Exited zombies own no listeners/workers.
use std::{
    fs,
    time::{Duration, Instant},
};

pub fn live_group_members(group: u32) -> Vec<u32> {
    let mut members = Vec::new();
    for entry in fs::read_dir("/proc").expect("process inventory") {
        let entry = entry.expect("process inventory entry");
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let stat = match fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => panic!("cannot inspect process {pid}: {error}"),
        };
        let (_, fields) = stat.rsplit_once(") ").expect("process stat fields");
        let fields: Vec<_> = fields.split_whitespace().collect();
        assert!(fields.len() > 2, "incomplete process stat");
        if fields[2].parse::<u32>().expect("process group") == group
            && !matches!(fields[0], "Z" | "X")
        {
            members.push(pid);
        }
    }
    members.sort();
    members
}

pub fn assert_group_exited(group: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = live_group_members(group);
        if remaining.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "command group {group} retains live descendants: {remaining:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
