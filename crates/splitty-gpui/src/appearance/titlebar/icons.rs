//! Small SVG-only icon-theme adapter for the four window controls. Lookup and
//! validation happen off the UI thread; renderers receive only retained bytes.
use gpui_kit::{Global, WindowButton};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct DesktopWindowIcons {
    // Close, minimize, maximize, restore.
    icons: [Option<Arc<[u8]>>; 4],
}
impl Global for DesktopWindowIcons {}

impl DesktopWindowIcons {
    pub fn svg(&self, button: WindowButton, maximized: bool) -> Option<&[u8]> {
        self.icons[match button {
            WindowButton::Close => 0,
            WindowButton::Minimize => 1,
            WindowButton::Maximize if maximized => 3,
            WindowButton::Maximize => 2,
        }]
        .as_deref()
    }

    #[cfg(target_os = "linux")]
    pub fn load(theme: &str, renderer: &gpui_kit::SvgRenderer) -> Self {
        lookup::load(theme, &lookup::search_roots(), renderer)
    }
}

#[cfg(any(target_os = "linux", test))]
mod lookup {
    use super::DesktopWindowIcons;
    use gpui_kit::SvgRenderer;
    use std::{
        collections::{BTreeMap, HashSet},
        fs::File,
        io::Read,
        path::{Component, Path, PathBuf},
    };

    const MAX_SVG_BYTES: usize = 64 * 1024;
    const MAX_INDEX_BYTES: usize = 256 * 1024;
    const MAX_THEMES: usize = 32;
    const MAX_DIRECTORIES: usize = 512;
    type Sections = BTreeMap<String, BTreeMap<String, String>>;

    #[cfg(target_os = "linux")]
    pub(super) fn search_roots() -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut roots = Vec::new();
        if let Some(home) = &home {
            roots.push(home.join(".icons"));
        }
        if let Some(data) = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| home.map(|home| home.join(".local/share")))
        {
            roots.push(data.join("icons"));
        }
        let data = std::env::var_os("XDG_DATA_DIRS")
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        roots.extend(
            std::env::split_paths(&data)
                .filter(|path| path.is_absolute())
                .map(|path| path.join("icons")),
        );
        roots
    }

    pub(super) fn load(
        theme: &str,
        roots: &[PathBuf],
        renderer: &SvgRenderer,
    ) -> DesktopWindowIcons {
        let icons = [
            "window-close-symbolic",
            "window-minimize-symbolic",
            "window-maximize-symbolic",
            "window-restore-symbolic",
        ]
        .map(|name| {
            [theme, "hicolor", "Adwaita"]
                .into_iter()
                .find_map(|theme| lookup(theme, name, roots, renderer, &mut HashSet::new()))
                .map(Into::into)
        });
        DesktopWindowIcons { icons }
    }

    fn read_file(path: &Path, limit: usize) -> Option<Vec<u8>> {
        let file = File::open(path).ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || metadata.len() > limit as u64 {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1).read_to_end(&mut bytes).ok()?;
        (bytes.len() <= limit).then_some(bytes)
    }

    fn relative_path(value: &str) -> bool {
        !value.is_empty()
            && !value.contains('\\')
            && Path::new(value)
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
    }

    fn read_index(path: &Path) -> Option<Sections> {
        let bytes = read_file(path, MAX_INDEX_BYTES)?;
        let text = std::str::from_utf8(&bytes).ok()?;
        let mut sections = Sections::new();
        let mut section = "";
        for line in text.lines().map(str::trim) {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                section = name;
            } else if let Some((key, value)) = line.split_once('=') {
                sections
                    .entry(section.to_owned())
                    .or_default()
                    .insert(key.trim().to_owned(), value.trim().to_owned());
            }
        }
        sections.contains_key("Icon Theme").then_some(sections)
    }

    // These are vector symbols displayed at 16 logical pixels on every output
    // scale. Prefer directories designed for that size, retaining theme order
    // and then base-directory precedence for equally suitable candidates.
    fn distance(section: Option<&BTreeMap<String, String>>) -> u32 {
        let Some(section) = section else {
            return u32::MAX;
        };
        let number = |key: &str, fallback| {
            section
                .get(key)
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(fallback)
        };
        let size = number("Size", 16);
        let scale = number("Scale", 1).max(1);
        let (low, high) = match section.get("Type").map(String::as_str) {
            Some("Fixed") => (size, size),
            Some("Scalable") => (number("MinSize", size), number("MaxSize", size)),
            _ => {
                let threshold = number("Threshold", 2);
                (
                    size.saturating_sub(threshold),
                    size.saturating_add(threshold),
                )
            }
        };
        let low = low.saturating_mul(scale);
        let high = high.saturating_mul(scale).max(low);
        16_u32.abs_diff(16_u32.clamp(low, high))
    }

    fn lookup(
        theme: &str,
        name: &str,
        roots: &[PathBuf],
        renderer: &SvgRenderer,
        visited: &mut HashSet<String>,
    ) -> Option<Vec<u8>> {
        if theme.len() > 256
            || theme.contains('/')
            || !relative_path(theme)
            || visited.len() >= MAX_THEMES
            || !visited.insert(theme.to_owned())
        {
            return None;
        }
        let index = roots
            .iter()
            .find_map(|root| read_index(&root.join(theme).join("index.theme")))?;
        let header = index.get("Icon Theme")?;
        let mut directories: Vec<_> = ["Directories", "ScaledDirectories"]
            .into_iter()
            .filter_map(|key| header.get(key))
            .flat_map(|value| value.split(',').map(str::trim))
            .filter(|dir| relative_path(dir))
            .take(MAX_DIRECTORIES)
            .collect();
        directories.sort_by_key(|dir| distance(index.get(*dir)));
        for dir in directories {
            for root in roots {
                let path = root.join(theme).join(dir).join(format!("{name}.svg"));
                if let Some(bytes) = read_file(&path, MAX_SVG_BYTES) {
                    if renderer.parse_svg(&bytes).is_ok() {
                        return Some(bytes);
                    }
                }
            }
        }
        header
            .get("Inherits")?
            .split(',')
            .map(str::trim)
            .take(MAX_THEMES)
            .find_map(|parent| lookup(parent, name, roots, renderer, visited))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use gpui_kit::WindowButton;
        use std::{fs, sync::Arc};

        const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\"><path d=\"M4 4h8v8H4z\"/></svg>";

        fn theme(root: &Path, name: &str, parents: &str) -> PathBuf {
            let path = root.join(name);
            fs::create_dir_all(path.join("symbolic")).unwrap();
            fs::write(path.join("index.theme"), format!("[Icon Theme]\nDirectories=symbolic\nInherits={parents}\n[symbolic]\nSize=16\nType=Scalable\nMinSize=8\nMaxSize=64\n")).unwrap();
            path.join("symbolic")
        }

        #[test]
        fn loads_overrides_recursive_inheritance_and_distinct_restore_without_bundled_artwork() {
            let tmp = tempfile::tempdir().unwrap();
            let user = tmp.path().join("user");
            let system = tmp.path().join("system");
            let active = theme(&system, "Active", "Parent");
            theme(&system, "Parent", "Grandparent");
            let ancestor = theme(&system, "Grandparent", "Active");
            let fallback = theme(&system, "Adwaita", "");
            let user_active = user.join("Active/symbolic");
            fs::create_dir_all(&user_active).unwrap();
            let custom = SVG.replace("M4 4h8v8H4z", "M1 1h14v14H1z");
            fs::write(active.join("window-close-symbolic.svg"), SVG).unwrap();
            fs::write(user_active.join("window-close-symbolic.svg"), &custom).unwrap();
            fs::write(ancestor.join("window-maximize-symbolic.svg"), SVG).unwrap();
            fs::write(fallback.join("window-restore-symbolic.svg"), &custom).unwrap();
            let renderer = SvgRenderer::new(Arc::new(()));
            let icons = load("Active", &[user, system], &renderer);
            assert_eq!(
                icons.svg(WindowButton::Close, false),
                Some(custom.as_bytes())
            );
            assert_eq!(
                icons.svg(WindowButton::Maximize, false),
                Some(SVG.as_bytes())
            );
            assert_eq!(
                icons.svg(WindowButton::Maximize, true),
                Some(custom.as_bytes())
            );
            assert!(icons.svg(WindowButton::Minimize, false).is_none());
        }

        #[test]
        fn invalid_or_oversized_symbols_fall_back_and_reloading_observes_changes() {
            let tmp = tempfile::tempdir().unwrap();
            let active = theme(tmp.path(), "Active", "");
            let fallback = theme(tmp.path(), "Adwaita", "");
            fs::write(active.join("window-close-symbolic.svg"), b"not svg").unwrap();
            fs::write(
                active.join("window-minimize-symbolic.svg"),
                vec![b' '; MAX_SVG_BYTES + 1],
            )
            .unwrap();
            for name in ["close", "minimize"] {
                fs::write(fallback.join(format!("window-{name}-symbolic.svg")), SVG).unwrap();
            }
            let roots = [tmp.path().to_owned()];
            let renderer = SvgRenderer::new(Arc::new(()));
            let icons = load("Active", &roots, &renderer);
            assert_eq!(icons.svg(WindowButton::Close, false), Some(SVG.as_bytes()));
            assert_eq!(
                icons.svg(WindowButton::Minimize, false),
                Some(SVG.as_bytes())
            );
            let changed = SVG.replace("M4 4h8v8H4z", "M2 2h12v12H2z");
            fs::write(active.join("window-close-symbolic.svg"), &changed).unwrap();
            assert_eq!(
                load("Active", &roots, &renderer).svg(WindowButton::Close, false),
                Some(changed.as_bytes())
            );
            assert_eq!(
                load("../Active", &roots, &renderer).svg(WindowButton::Close, false),
                Some(SVG.as_bytes())
            );
        }
    }
}
