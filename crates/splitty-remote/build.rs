//! Builds the server archives named by `SPLITTY_REMOTE_PAYLOAD_X86_64` and
//! `SPLITTY_REMOTE_PAYLOAD_AARCH64` (Linux) and by
//! `SPLITTY_REMOTE_PAYLOAD_DARWIN_X86_64` and
//! `SPLITTY_REMOTE_PAYLOAD_DARWIN_AARCH64` (macOS) into the crate, with their
//! checksums computed now. Without them, the table is empty (development
//! builds).

use std::env;
use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// Kernel as `uname -s` names it, architecture, and the variable naming the
/// archive.
const PAYLOADS: [(&str, &str, &str); 4] = [
    ("Linux", "x86_64", "SPLITTY_REMOTE_PAYLOAD_X86_64"),
    ("Linux", "aarch64", "SPLITTY_REMOTE_PAYLOAD_AARCH64"),
    ("Darwin", "x86_64", "SPLITTY_REMOTE_PAYLOAD_DARWIN_X86_64"),
    ("Darwin", "aarch64", "SPLITTY_REMOTE_PAYLOAD_DARWIN_AARCH64"),
];

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let mut table = String::from(
        "/// Server archives built into this client: kernel, architecture, archive,\n\
         /// SHA-256.\n\
         pub static BUILT_IN: &[(&str, &str, &[u8], &str)] = &[\n",
    );
    for (kernel, architecture, variable) in PAYLOADS {
        println!("cargo:rerun-if-env-changed={variable}");
        let Some(path) = env::var_os(variable).filter(|value| !value.is_empty()) else {
            continue;
        };
        let path = PathBuf::from(path);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = fs::read(&path)
            .unwrap_or_else(|e| panic!("{variable}: cannot read {}: {e}", path.display()));
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let copy = out.join(format!("payload-{kernel}-{architecture}.tar.gz"));
        fs::write(&copy, &bytes).expect("copy the payload into OUT_DIR");
        table.push_str(&format!(
            "    ({kernel:?}, {architecture:?}, include_bytes!({copy:?}), {sha256:?}),\n"
        ));
    }
    table.push_str("];\n");
    fs::write(out.join("payloads.rs"), table).expect("write payloads.rs");
}
