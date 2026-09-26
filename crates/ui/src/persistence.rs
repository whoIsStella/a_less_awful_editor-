//! Blocking single-file operations. Call only on the background executor.
//! Checks are conservative, but are not a filesystem transaction with other writers.
use ale_editor_core::{EditorBuffer, TextSnapshot};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiskVersion {
    bytes: Vec<u8>,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, u32, u32, u32, u64),
}

impl DiskVersion {
    fn new(bytes: Vec<u8>, meta: &Metadata) -> Self {
        Self {
            bytes,
            modified: meta.modified().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (
                    meta.dev(),
                    meta.ino(),
                    meta.mode(),
                    meta.uid(),
                    meta.gid(),
                    meta.nlink(),
                )
            },
        }
    }
}

pub(crate) struct Loaded {
    pub path: PathBuf,
    pub buffer: EditorBuffer,
    pub version: DiskVersion,
}

#[derive(Debug)]
pub(crate) enum SaveError {
    Conflict(Option<DiskVersion>),
    Failed(String),
}

impl From<std::io::Error> for SaveError {
    fn from(error: std::io::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

fn checked_path(path: &Path) -> Result<PathBuf, String> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("Paths containing '..' are unsupported; choose an absolute path.".into());
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(
                "Symbolic links (including parent directories) are unsupported. Choose the real path.".into()),
            Ok(_) => {},
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(path)
}

fn read_version(path: &Path) -> Result<Option<(DiskVersion, Metadata)>, String> {
    checked_path(path)?;
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
        Ok(meta) if !meta.is_file() => return Err("Only regular files are supported.".into()),
        Ok(_) => {}
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    let before = file.metadata().map_err(|e| e.to_string())?;
    if !before.is_file() {
        return Err("Only regular files are supported.".into());
    }
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Files larger than 16 MiB are not supported in this milestone.".into());
    }
    let after = file.metadata().map_err(|e| e.to_string())?;
    if DiskVersion::new(Vec::new(), &before) != DiskVersion::new(Vec::new(), &after)
        || after.len() != bytes.len() as u64
    {
        return Err("The file changed while being read. Try again.".into());
    }
    Ok(Some((DiskVersion::new(bytes, &after), after)))
}

pub(crate) fn load(path: &Path) -> Result<Loaded, String> {
    let path = checked_path(path)?;
    let (version, _) = read_version(&path)?.ok_or("The selected file no longer exists.")?;
    let text = std::str::from_utf8(&version.bytes).map_err(
        |_| "Unsupported encoding: only strict UTF-8 is supported; no bytes were replaced.",
    )?;
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(
            "Binary/control-character input is unsupported; the current document is unchanged."
                .into(),
        );
    }
    Ok(Loaded {
        path,
        buffer: EditorBuffer::with_text(text),
        version,
    })
}

/// Expected=None means create only. Conflicts return the observed version so an
/// explicit overwrite decision can be tied to it, then checked again on retry.
pub(crate) fn save(
    path: &Path,
    snapshot: &TextSnapshot,
    expected: Option<&DiskVersion>,
) -> Result<DiskVersion, SaveError> {
    save_impl(path, snapshot, expected, || {})
}

fn save_impl(
    path: &Path,
    snapshot: &TextSnapshot,
    expected: Option<&DiskVersion>,
    before_commit: impl FnOnce(),
) -> Result<DiskVersion, SaveError> {
    let path = checked_path(path).map_err(SaveError::Failed)?;
    let current = read_version(&path).map_err(SaveError::Failed)?;
    if current.as_ref().map(|(version, _)| version) != expected {
        return Err(SaveError::Conflict(current.map(|(v, _)| v)));
    }
    // Metadata preservation has only been implemented for Linux in this milestone.
    #[cfg(not(target_os = "linux"))]
    if current.is_some() {
        return Err(SaveError::Failed("Overwriting existing files is currently supported only on Linux. Use Save As with a new destination.".into()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| SaveError::Failed("No parent directory.".into()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    if let Some((_, metadata)) = &current {
        if metadata.permissions().readonly() {
            return Err(SaveError::Failed(
                "File is read-only. Use Save As to choose another destination.".into(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let ours = temporary.as_file().metadata()?;
            if metadata.nlink() != 1
                || metadata.mode() & 0o7000 != 0
                || metadata.uid() != ours.uid()
                || metadata.gid() != ours.gid()
            {
                return Err(SaveError::Failed("Hard links, special permission bits, or differing ownership are unsupported. Use Save As.".into()));
            }
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            let file = File::open(&path)?;
            // SAFETY: a valid owned descriptor and null buffer with size zero
            // ask Linux for the attribute-list size without writing memory.
            let count = unsafe { libc::flistxattr(file.as_raw_fd(), std::ptr::null_mut(), 0) };
            if count != 0 {
                return Err(SaveError::Failed("Files with extended attributes/ACLs (or unreadable attributes) are unsupported. Use Save As.".into()));
            }
        }
    }
    snapshot.write_to(temporary.as_file_mut())?;
    temporary.flush()?;
    if temporary.as_file().metadata()?.len() > MAX_BYTES {
        return Err(SaveError::Failed(
            "Saving more than 16 MiB is unsupported. Buffer retained.".into(),
        ));
    }
    if let Some((_, metadata)) = &current {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())?;
    }
    temporary.as_file().sync_all()?;
    let mut bytes = Vec::new();
    snapshot.write_to(&mut bytes)?;
    if std::str::from_utf8(&bytes)
        .expect("rope contains UTF-8")
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(SaveError::Failed("Unsupported control characters in the buffer. Remove them before saving; disk is unchanged.".into()));
    }
    let version = DiskVersion::new(bytes, &temporary.as_file().metadata()?);
    before_commit();
    let latest = read_version(&path)
        .map_err(SaveError::Failed)?
        .map(|(v, _)| v);
    if latest.as_ref() != expected {
        return Err(SaveError::Conflict(latest));
    }
    if expected.is_none() {
        temporary
            .persist_noclobber(&path)
            .map_err(|e| SaveError::Failed(e.error.to_string()))?;
    } else {
        temporary
            .persist(&path)
            .map_err(|e| SaveError::Failed(e.error.to_string()))?;
    }
    #[cfg(unix)]
    File::open(parent)?.sync_all().map_err(|e| SaveError::Failed(format!(
        "File replaced, but directory sync failed ({e}). Durability is uncertain; buffer remains dirty.")))?;
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(text: &str) -> TextSnapshot {
        EditorBuffer::with_text(text).text_snapshot()
    }

    #[test]
    fn unicode_lf_crlf_mixed_and_bom_round_trip_exactly() {
        let dir = tempfile::tempdir().unwrap();
        for text in [
            "",
            "a\nb\n",
            "a\r\nb\r\n",
            "\u{feff}e\u{301}👩‍💻\r\n東京\nx\r",
        ] {
            let path = dir.path().join("roundtrip");
            fs::write(&path, text).unwrap();
            let mut loaded = load(&path).unwrap();
            loaded.buffer.select_all();
            loaded.buffer.insert("replacement");
            loaded.buffer.undo();
            save(&path, &loaded.buffer.text_snapshot(), Some(&loaded.version)).unwrap();
            assert_eq!(fs::read(&path).unwrap(), text.as_bytes());
        }
    }

    #[test]
    fn save_as_is_create_only_until_explicit_overwrite_of_observed_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new");
        save(&path, &snapshot("one"), None).unwrap();
        let Err(SaveError::Conflict(observed)) = save(&path, &snapshot("two"), None) else {
            panic!("must conflict")
        };
        assert_eq!(fs::read(&path).unwrap(), b"one");
        save(&path, &snapshot("two"), observed.as_ref()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
    }

    #[test]
    fn external_change_deletion_and_second_change_require_new_decision() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, "initial").unwrap();
        let loaded = load(&path).unwrap();
        fs::write(&path, "outside").unwrap();
        let Err(SaveError::Conflict(observed)) =
            save(&path, &snapshot("mine"), Some(&loaded.version))
        else {
            panic!("must conflict")
        };
        fs::write(&path, "changed again").unwrap();
        assert!(matches!(
            save(&path, &snapshot("mine"), observed.as_ref()),
            Err(SaveError::Conflict(_))
        ));
        fs::remove_file(&path).unwrap();
        assert!(matches!(
            save(&path, &snapshot("mine"), Some(&loaded.version)),
            Err(SaveError::Conflict(None))
        ));
        save(&path, &snapshot("mine"), None).unwrap();
    }

    #[test]
    fn late_conflict_preserves_destination_and_cleans_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, "initial").unwrap();
        let loaded = load(&path).unwrap();
        let result = save_impl(&path, &snapshot("mine"), Some(&loaded.version), || {
            fs::write(&path, "racing writer").unwrap()
        });
        assert!(matches!(result, Err(SaveError::Conflict(_))));
        assert_eq!(fs::read(&path).unwrap(), b"racing writer");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn invalid_utf8_binary_directory_and_missing_file_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        for bytes in [b"a\0b".as_slice(), b"\xff\xfe", b"\x01"] {
            fs::write(&path, bytes).unwrap();
            assert!(load(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        assert!(load(dir.path()).is_err());
        assert!(load(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn failed_save_keeps_buffer_dirty_and_destination_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, "original").unwrap();
        let mut loaded = load(&path).unwrap();
        loaded.buffer.insert("edit");
        let saved = loaded.buffer.text_snapshot();
        assert!(save(&dir.path().join("missing-parent/file"), &saved, None).is_err());
        assert!(loaded.buffer.is_dirty());
        assert_eq!(fs::read(&path).unwrap(), b"original");
    }

    #[test]
    fn save_in_flight_uses_a_defined_snapshot_and_undo_finds_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        let mut buffer = EditorBuffer::new();
        buffer.insert("captured");
        let saved = buffer.text_snapshot();
        save_impl(&path, &saved, None, || buffer.insert(" newer")).unwrap();
        buffer.mark_saved(&saved);
        assert_eq!(fs::read(&path).unwrap(), b"captured");
        assert!(buffer.is_dirty());
        buffer.undo();
        assert!(!buffer.is_dirty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn readonly_hardlink_symlink_and_symlink_parent_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, "original").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let loaded = load(&path).unwrap();
        assert!(matches!(
            save(&path, &snapshot("new"), Some(&loaded.version)),
            Err(SaveError::Failed(_))
        ));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let loaded = load(&path).unwrap();
        save(&path, &snapshot("original"), Some(&loaded.version)).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        fs::hard_link(&path, dir.path().join("hard")).unwrap();
        let loaded = load(&path).unwrap();
        assert!(matches!(
            save(&path, &snapshot("new"), Some(&loaded.version)),
            Err(SaveError::Failed(_))
        ));
        let link = dir.path().join("link");
        symlink(&path, &link).unwrap();
        assert!(load(&link).is_err());
        assert!(save(&link, &snapshot("new"), None).is_err());
        let parent = dir.path().join("parent");
        symlink(dir.path(), &parent).unwrap();
        assert!(save(&parent.join("new"), &snapshot("new"), None).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
    }
}
