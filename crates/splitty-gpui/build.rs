use std::{env, fs, path::PathBuf};

const VERSION: &str = "1.0.0";

fn main() {
    bundle_themes();
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

/// Embed the theme files of `themes/` as two tables of `(name, text)`: the UI
/// theme sets (`ui/**/*.json`) and the terminal palettes (`terminal/*/*`).
/// Splitty's own files come first, so they keep their names against a
/// third-party file of the same name.
fn bundle_themes() {
    let root =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets it")).join("themes");
    println!("cargo:rerun-if-changed={}", root.display());
    let files = |directory: PathBuf| -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
            .map(|entry| entry.expect("readable theme directory").path())
            .filter(|path| path.is_file())
            .collect();
        files.sort();
        files
    };
    let name = |path: &PathBuf| {
        path.file_name()
            .unwrap()
            .to_str()
            .expect("UTF-8 name")
            .to_owned()
    };
    let entry = |name: &str, path: &PathBuf| {
        format!(
            "    ({name:?}, include_str!({:?})),\n",
            path.to_str().expect("UTF-8 path")
        )
    };
    let mut ui = String::new();
    for path in files(root.join("ui"))
        .into_iter()
        .chain(files(root.join("ui/gpui-kit")))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
    {
        ui.push_str(&entry(&name(&path), &path));
    }
    let mut palettes = String::new();
    for path in files(root.join("terminal/splitty"))
        .into_iter()
        .chain(files(root.join("terminal/iterm2")))
        .filter(|path| !matches!(name(path).as_str(), "LICENSE" | "SOURCE.md"))
    {
        palettes.push_str(&entry(&name(&path), &path));
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    fs::write(
        out.join("bundled_themes.rs"),
        format!(
            "pub static UI_THEME_SETS: &[(&str, &str)] = &[\n{ui}];\n\
             pub static TERMINAL_PALETTES: &[(&str, &str)] = &[\n{palettes}];\n"
        ),
    )
    .expect("write the bundled theme tables");
}
