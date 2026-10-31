//! The store of images pasted into a pane (`save_image`).
//!
//! A client sends an image as base64; the server keeps it as a file only its
//! owner can read, in `splitty/paste/` of the user's cache directory, and
//! answers with the file's absolute path, which the client pastes into the
//! pane. The server picks the file's name and its extension from the image's
//! leading bytes: no part of the request names a file.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use rand::Rng;

/// The largest image a client may paste, decoded.
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// The longest base64 text that can hold an image of [`MAX_IMAGE_BYTES`].
/// Longer data is refused before it is decoded.
pub const MAX_ENCODED_LEN: usize = MAX_IMAGE_BYTES.div_ceil(3) * 4;

/// Pasted images older than this are removed by the next save.
pub(crate) const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// How many earlier pasted images a save keeps at most.
pub(crate) const MAX_KEPT: usize = 100;

/// The start of every pasted image's file name, and of only those.
const PREFIX: &str = "paste-";

/// How many names a save tries when the one it picked is taken.
pub(crate) const NAME_ATTEMPTS: usize = 4;

/// Why a pasted image was not stored. The message goes to the client.
#[derive(Debug, thiserror::Error)]
pub enum PasteError {
    #[error("the image is not valid base64")]
    InvalidBase64,
    #[error("the image is larger than 10 MiB")]
    TooLarge,
    #[error("not a PNG, JPEG, GIF or WebP image")]
    NotAnImage,
    #[error("the server has no cache directory for pasted images")]
    NoDirectory,
    #[error("the paste directory {} is a symbolic link", .0.display())]
    SymlinkedDirectory(PathBuf),
    #[error("the paste directory {} is not valid UTF-8", .0.display())]
    NonUtf8Directory(PathBuf),
    #[error("could not store the image: {0}")]
    Io(#[from] io::Error),
}

/// `splitty/paste/` of the user's cache directory (`$XDG_CACHE_HOME`, else
/// `~/.cache`; `~/Library/Caches` on macOS), or `None` without one.
pub fn default_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|cache| cache.join("splitty").join("paste"))
}

/// Decode `encoded`, an image as standard base64, and store it as a new file
/// in `dir`, after removing the old pasted images there. Returns the file's
/// absolute path, which is valid UTF-8.
///
/// Blocking: call it off the async runtime's workers.
pub fn save_base64(dir: &Path, encoded: &[u8]) -> Result<PathBuf, PasteError> {
    if encoded.len() > MAX_ENCODED_LEN {
        return Err(PasteError::TooLarge);
    }
    let image = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| PasteError::InvalidBase64)?;
    save(dir, &image)
}

fn save(dir: &Path, image: &[u8]) -> Result<PathBuf, PasteError> {
    if image.len() > MAX_IMAGE_BYTES {
        return Err(PasteError::TooLarge);
    }
    let extension = image_extension(image).ok_or(PasteError::NotAnImage)?;
    let dir = prepare_dir(dir)?;
    remove_old(&dir, SystemTime::now());
    write_new(&dir, extension, image)
}

/// The file extension of a PNG, JPEG, GIF or WebP image, from its leading
/// bytes; `None` for anything else.
fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
        Some("webp")
    } else {
        None
    }
}

/// Make `dir` absolute and create it with mode 0700, refusing a directory
/// that is a symbolic link: following one would store the image, and set the
/// mode, wherever the link points.
pub(crate) fn prepare_dir(dir: &Path) -> Result<PathBuf, PasteError> {
    let dir = std::path::absolute(dir)?;
    if dir.to_str().is_none() {
        return Err(PasteError::NonUtf8Directory(dir));
    }
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(PasteError::SymlinkedDirectory(dir));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    crate::config::ensure_private_dir(&dir)?;
    Ok(dir)
}

/// Remove the pasted images in `dir` that are older than [`MAX_AGE`], then
/// all but the newest [`MAX_KEPT`]. Only regular files whose names start with
/// [`PREFIX`] count. A failure is logged and does not stop the save.
fn remove_old(dir: &Path, now: SystemTime) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(dir = %dir.display(), %error, "cannot list the pasted images");
            return;
        }
    };
    let mut kept: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(dir = %dir.display(), %error, "cannot list the pasted images");
                continue;
            }
        };
        if !entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(PREFIX.as_bytes())
        {
            continue;
        }
        let path = entry.path();
        // Does not follow a link, so a link is not a regular file here.
        let modified = match entry.metadata() {
            Ok(meta) if !meta.is_file() => continue,
            Ok(meta) => meta.modified(),
            Err(error) => Err(error),
        };
        match modified {
            Ok(modified) if now.duration_since(modified).unwrap_or_default() > MAX_AGE => {
                remove_pasted(&path);
            }
            Ok(modified) => kept.push((modified, path)),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "cannot read the age of a pasted image");
            }
        }
    }
    if kept.len() > MAX_KEPT {
        // Newest first; the name, which starts with the time, breaks ties.
        kept.sort_unstable_by(|a, b| b.cmp(a));
        for (_, path) in kept.drain(MAX_KEPT..) {
            remove_pasted(&path);
        }
    }
}

fn remove_pasted(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        tracing::warn!(path = %path.display(), %error, "cannot remove an old pasted image");
    }
}

/// Write `image` to a new file in `dir` under a name of its own; a name that
/// is taken is never reused.
fn write_new(dir: &Path, extension: &str, image: &[u8]) -> Result<PathBuf, PasteError> {
    for _ in 1..NAME_ATTEMPTS {
        match write_file(dir, extension, image) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            result => return result.map_err(PasteError::from),
        }
    }
    write_file(dir, extension, image).map_err(PasteError::from)
}

fn write_file(dir: &Path, extension: &str, image: &[u8]) -> io::Result<PathBuf> {
    let path = dir.join(file_name(SystemTime::now(), extension));
    let mut options = fs::OpenOptions::new();
    // `create_new` fails on any existing entry, a link included, so the
    // image never lands where a link points.
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    if let Err(error) = file.write_all(image) {
        drop(file);
        remove_pasted(&path);
        return Err(error);
    }
    Ok(path)
}

/// `paste-<UTC yyyymmdd-hhmmss>-<8 random hex digits>.<extension>`.
fn file_name(now: SystemTime, extension: &str) -> String {
    let secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let mut random = [0u8; 4];
    rand::thread_rng().fill(&mut random);
    format!(
        "{PREFIX}{}-{}.{extension}",
        utc_stamp(secs),
        hex::encode(random)
    )
}

/// `yyyymmdd-hhmmss` in UTC for `secs` seconds after the Unix epoch.
pub(crate) fn utc_stamp(secs: u64) -> String {
    let (year, month, day) = civil_from_days(secs / 86_400);
    let time = secs % 86_400;
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        time / 3_600,
        time % 3_600 / 60,
        time % 60
    )
}

/// The Gregorian date `days` days after 1970-01-01 as (year, month, day), by
/// Howard Hinnant's `civil_from_days` for days that are not negative.
pub fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const JPEG: &[u8] = b"\xFF\xD8\xFF\xE0\0\x10JFIF\0";
    const GIF87: &[u8] = b"GIF87a\x01\0\x01\0";
    const GIF89: &[u8] = b"GIF89a\x01\0\x01\0";
    const WEBP: &[u8] = b"RIFF\x24\0\0\0WEBPVP8 ";

    fn encode(bytes: &[u8]) -> Vec<u8> {
        STANDARD.encode(bytes).into_bytes()
    }

    /// The pasted images in `dir`, by name; none for a missing directory.
    fn pasted(dir: &Path) -> Vec<String> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with(PREFIX))
            .collect();
        names.sort();
        names
    }

    fn file_aged(dir: &Path, name: &str, age: Duration) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, PNG).unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
        path
    }

    /// terminal-sessions, Image Upload: a PNG is stored owner-only under a
    /// name the server picks, with its extension, in a 0700 directory.
    #[test]
    fn stores_a_png_owner_only_with_its_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("splitty").join("paste");
        let image = [PNG, &[7u8; 200 * 1024]].concat();

        let path = save_base64(&dir, &encode(&image)).unwrap();

        assert!(path.is_absolute(), "{path:?}");
        assert_eq!(path.parent(), Some(dir.as_path()));
        assert_eq!(fs::read(&path).unwrap(), image);
        let name = path.file_name().unwrap().to_str().unwrap();
        let (stem, extension) = name.rsplit_once('.').unwrap();
        assert_eq!(extension, "png");
        let parts: Vec<&str> = stem.split('-').collect();
        assert_eq!(parts.len(), 4, "{name}");
        assert_eq!(parts[0], "paste");
        assert_eq!(parts[1].len(), 8, "{name}");
        assert_eq!(parts[2].len(), 6, "{name}");
        assert!(parts[1..3]
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_digit())));
        assert_eq!(parts[3].len(), 8, "{name}");
        assert!(parts[3]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&dir), 0o700);
        }
    }

    #[test]
    fn detects_each_image_type_by_its_leading_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        for (image, expected) in [
            (PNG, "png"),
            (JPEG, "jpg"),
            (GIF87, "gif"),
            (GIF89, "gif"),
            (WEBP, "webp"),
        ] {
            assert_eq!(image_extension(image), Some(expected));
            let path = save_base64(tmp.path(), &encode(image)).unwrap();
            assert_eq!(path.extension().unwrap(), expected);
            assert_eq!(fs::read(&path).unwrap(), image);
        }

        for not_an_image in [
            b"".as_slice(),
            b"\x89PNG\r\n",
            b"\xFF\xD8",
            b"GIF88a\x01\0",
            b"RIFF\x24\0\0\0WAVEfmt ",
            b"RIFF\x24\0\0\0WEB",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
        ] {
            assert_eq!(image_extension(not_an_image), None, "{not_an_image:?}");
        }
    }

    /// terminal-sessions, Image Upload, Not an image.
    #[test]
    fn refuses_what_is_not_an_image_and_stores_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("paste");

        let result = save_base64(&dir, &encode(b"#!/bin/sh\nrm -rf ~\n"));

        assert!(matches!(result, Err(PasteError::NotAnImage)), "{result:?}");
        assert!(!dir.exists());
    }

    /// terminal-sessions, Image Upload, Too large: refused once decoded, and
    /// text too long for 10 MiB before it is decoded.
    #[test]
    fn refuses_an_image_larger_than_10_mib() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("paste");
        let mut image = PNG.to_vec();
        image.resize(MAX_IMAGE_BYTES + 1, 0);
        let encoded = encode(&image);
        assert!(encoded.len() <= MAX_ENCODED_LEN);

        let result = save_base64(&dir, &encoded);
        assert!(matches!(result, Err(PasteError::TooLarge)), "{result:?}");

        // Not even base64: refused for its length, so it was never decoded.
        let result = save_base64(&dir, &vec![b'!'; MAX_ENCODED_LEN + 1]);
        assert!(matches!(result, Err(PasteError::TooLarge)), "{result:?}");
        assert!(!dir.exists());

        image.truncate(MAX_IMAGE_BYTES);
        let path = save_base64(&dir, &encode(&image)).unwrap();
        assert_eq!(fs::metadata(path).unwrap().len(), 10 * 1024 * 1024);
    }

    #[test]
    fn refuses_invalid_base64_and_stores_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("paste");

        for invalid in [
            b"not base64!".as_slice(),
            b"iVBORw0KGgo=\n",
            b"iVBO$w0KGgo=",
        ] {
            let result = save_base64(&dir, invalid);
            assert!(
                matches!(result, Err(PasteError::InvalidBase64)),
                "{invalid:?}: {result:?}"
            );
        }
        assert!(!dir.exists());
    }

    /// terminal-sessions, Image Upload, Old pastes: only `paste-` files older
    /// than a day go.
    #[test]
    fn removes_pasted_images_older_than_a_day() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let day = Duration::from_secs(24 * 60 * 60);
        let old = file_aged(dir, "paste-20261013-100000-00000000.png", 2 * day);
        let recent = file_aged(dir, "paste-20261015-090000-00000000.png", day / 24);
        let other = file_aged(dir, "notes.png", 2 * day);
        fs::create_dir(dir.join("paste-folder")).unwrap();

        let path = save_base64(dir, &encode(PNG)).unwrap();

        assert!(!old.exists());
        assert!(recent.exists());
        assert!(other.exists());
        assert!(dir.join("paste-folder").is_dir());
        assert!(path.exists());
    }

    /// A save keeps the newest 100 earlier pasted images: of 101, the oldest
    /// goes.
    #[test]
    fn keeps_the_newest_100_pasted_images() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        for minutes in 1..=101u64 {
            file_aged(
                dir,
                &format!("paste-{minutes:03}.png"),
                Duration::from_secs(60 * minutes),
            );
        }

        let path = save_base64(dir, &encode(PNG)).unwrap();

        let names = pasted(dir);
        assert_eq!(names.len(), 101);
        assert!(!names.contains(&"paste-101.png".to_string()));
        assert!(names.contains(&"paste-100.png".to_string()));
        assert!(path.exists());
    }

    /// A paste directory that is a link is refused, and nothing is written
    /// or changed where it points.
    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_directory() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("elsewhere");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let old = file_aged(
            &target,
            "paste-old.png",
            Duration::from_secs(3 * 24 * 60 * 60),
        );
        let dir = tmp.path().join("paste");
        std::os::unix::fs::symlink(&target, &dir).unwrap();

        let result = save_base64(&dir, &encode(PNG));

        assert!(
            matches!(result, Err(PasteError::SymlinkedDirectory(_))),
            "{result:?}"
        );
        assert_eq!(pasted(&target), ["paste-old.png"]);
        assert!(old.exists());
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn names_carry_the_utc_date_and_time() {
        assert_eq!(utc_stamp(0), "19700101-000000");
        assert_eq!(utc_stamp(951_782_400), "20000229-000000");
        assert_eq!(utc_stamp(1_792_067_696), "20261015-123456");
        assert_eq!(utc_stamp(4_102_444_799), "20991231-235959");
    }
}
