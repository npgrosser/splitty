use std::{env, fs, path::PathBuf};

const VERSION: &str = "1.0.0";

fn main() {
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_DIR");

    let prefix = env::var_os("GHOSTTY_VT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "GHOSTTY_VT_DIR is unset. Run `python3 scripts/bootstrap-libghostty-vt.py` \
                 and set GHOSTTY_VT_DIR to the printed prefix before building splitty-gpui"
            )
        });
    assert!(
        prefix.is_absolute(),
        "GHOSTTY_VT_DIR must be an absolute path: {}",
        prefix.display()
    );

    let version_header = prefix.join("include/ghostty/vt/version.h");
    let header = fs::read_to_string(&version_header).unwrap_or_else(|error| {
        panic!(
            "cannot read pinned libghostty-vt headers at {}: {error}",
            version_header.display()
        )
    });
    let expected = format!("#define GHOSTTY_VT_VERSION \"{VERSION}\"");
    assert!(
        header.lines().any(|line| line.trim() == expected),
        "{} must declare {expected}; rebuild the pinned libghostty-vt release",
        version_header.display()
    );

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo sets CARGO_CFG_TARGET_OS");
    let archive_name = match target_os.as_str() {
        "linux" | "macos" => "libghostty-vt.a",
        "windows" => "ghostty-vt-static.lib",
        other => panic!("libghostty-vt is not configured for target OS {other}"),
    };
    let archive = prefix.join("lib").join(archive_name);
    assert!(
        archive.is_file(),
        "missing pinned libghostty-vt static library at {}; run the bootstrap for this target",
        archive.display()
    );

    // macOS selected the adjacent dylib from prefix/lib despite a static link declaration.
    let static_dir =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR")).join("native-static");
    fs::create_dir_all(&static_dir).unwrap_or_else(|error| {
        panic!(
            "cannot create native static link directory {}: {error}",
            static_dir.display()
        )
    });
    let static_archive = static_dir.join(archive_name);
    fs::copy(&archive, &static_archive).unwrap_or_else(|error| {
        panic!(
            "cannot stage pinned libghostty-vt archive {} at {}: {error}",
            archive.display(),
            static_archive.display()
        )
    });

    println!("cargo:rerun-if-changed={}", version_header.display());
    println!("cargo:rerun-if-changed={}", archive.display());
    println!("cargo:rustc-link-search=native={}", static_dir.display());
    let name = if target_os == "windows" {
        "ghostty-vt-static"
    } else {
        "ghostty-vt"
    };
    println!("cargo:rustc-link-lib=static={name}");

    #[cfg(windows)]
    if target_os == "windows" {
        windows_resources();
    }
}

/// The executable's icon (resource 1, which the window and the taskbar use)
/// and its version information.
#[cfg(windows)]
fn windows_resources() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets it"));
    let icon = manifest.join("../splitty-tauri/icons/icon.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let version = env::var("CARGO_PKG_VERSION").expect("Cargo sets CARGO_PKG_VERSION");
    let numbers: Vec<&str> = version.split(['.', '-', '+']).take(3).collect();
    let numeric = format!("{},0", numbers.join(","));
    let script = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "ProductName", "Splitty\0"
      VALUE "FileDescription", "Splitty\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "ProductVersion", "{version}\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = icon.display().to_string().replace('\\', "/"),
    );
    let path =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR")).join("splitty.rc");
    fs::write(&path, script).expect("write the resource script");
    embed_resource::compile(&path, embed_resource::NONE)
        .manifest_required()
        .expect("compile the Windows resources");
}
