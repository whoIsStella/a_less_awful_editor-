//! Private, atomic recovery snapshots, scoped to a workspace.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

const MAX_SNAPSHOT_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Session {
    #[serde(with = "stored_path")]
    pub workspace: PathBuf,
    pub tabs: Vec<SessionTab>,
    pub active: usize,
    pub explorer_visible: bool,
    pub terminal_visible: bool,
    #[serde(default)]
    pub panel_visible: bool,
    #[serde(default)]
    pub panel: String,
    #[serde(default)]
    pub active_pane: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SessionTab {
    #[serde(with = "optional_path")]
    pub path: Option<PathBuf>,
    pub draft: String,
    pub base: Option<String>,
    pub anchor: usize,
    pub head: usize,
    pub dirty: bool,
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    session: Session,
}

mod stored_path {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::path::{Path, PathBuf};

    pub fn bytes(path: &Path) -> Vec<u8> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            path.as_os_str().as_bytes().to_vec()
        }
        #[cfg(not(unix))]
        {
            path.to_string_lossy().as_bytes().to_vec()
        }
    }
    pub fn from_bytes(bytes: Vec<u8>) -> PathBuf {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_vec(bytes))
        }
        #[cfg(not(unix))]
        {
            PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
        }
    }
    pub fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&bytes(path), serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
        Vec::<u8>::deserialize(deserializer).map(from_bytes)
    }
}
mod optional_path {
    use super::stored_path;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::path::PathBuf;

    pub fn serialize<S: Serializer>(
        path: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        path.as_ref()
            .map(|p| stored_path::bytes(p))
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        Option::<Vec<u8>>::deserialize(deserializer).map(|v| v.map(stored_path::from_bytes))
    }
}

fn reject_symlinks(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "Session storage cannot use a symbolic link: {}",
                    ancestor.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Cannot inspect session storage: {error}")),
        }
    }
    Ok(())
}

pub(crate) struct SessionStore {
    workspace: PathBuf,
    directory: PathBuf,
    _lock: File,
    writing: Mutex<()>,
}

impl SessionStore {
    pub fn open(workspace: &Path) -> Result<Self, String> {
        let base = std::env::var_os("XDG_STATE_HOME")
            .filter(|value| Path::new(value).is_absolute())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .ok_or("No state directory is configured.")?;
        Self::open_in(workspace, &base)
    }

    fn open_in(workspace: &Path, base: &Path) -> Result<Self, String> {
        let workspace = workspace
            .canonicalize()
            .map_err(|e| format!("Cannot open workspace: {e}"))?;
        #[cfg(unix)]
        let key = {
            use std::os::unix::ffi::OsStrExt;
            format!("{:x}", Sha256::digest(workspace.as_os_str().as_bytes()))
        };
        #[cfg(not(unix))]
        let key = format!(
            "{:x}",
            Sha256::digest(workspace.to_string_lossy().as_bytes())
        );
        let directory = base.join("a-less-awful-editor").join("sessions").join(key);
        reject_symlinks(&directory)?;
        fs::create_dir_all(&directory)
            .map_err(|e| format!("Cannot create session storage: {e}"))?;
        reject_symlinks(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let lock = options
            .open(directory.join("session.lock"))
            .map_err(|e| format!("Cannot open session lock: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // The descriptor remains open until the store is dropped.
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(format!(
                    "This workspace session is already in use: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        #[cfg(not(unix))]
        return Err("Workspace session locking is not supported on this platform.".into());
        #[cfg(unix)]
        Ok(Self {
            workspace,
            directory,
            _lock: lock,
            writing: Mutex::new(()),
        })
    }

    pub fn load(&self) -> Result<Option<Session>, String> {
        let _guard = self.writing.lock().map_err(|_| "Session lock failed.")?;
        let path = self.directory.join("session.json");
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Cannot read session: {e}")),
        };
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Session storage is not a regular file.".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_SNAPSHOT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
            return Err("Session snapshot is too large.".into());
        }
        let envelope: Envelope = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Cannot read session snapshot: {e}"))?;
        if envelope.version != 1 {
            return Err(format!(
                "Unsupported session snapshot version {}.",
                envelope.version
            ));
        }
        if envelope.session.workspace != self.workspace {
            return Err("Session workspace does not match.".into());
        }
        if envelope.session.tabs.len() > 32
            || (!envelope.session.tabs.is_empty()
                && envelope.session.active >= envelope.session.tabs.len())
        {
            return Err("Session tab list is invalid.".into());
        }
        Ok(Some(envelope.session))
    }

    pub fn save(&self, session: &Session) -> Result<(), String> {
        let _guard = self.writing.lock().map_err(|_| "Session lock failed.")?;
        if session.workspace != self.workspace {
            return Err("Session workspace does not match.".into());
        }
        if session.tabs.len() > 32
            || (!session.tabs.is_empty() && session.active >= session.tabs.len())
        {
            return Err("Session tab list is invalid.".into());
        }
        #[derive(Serialize)]
        struct EnvelopeRef<'a> {
            version: u32,
            session: &'a Session,
        }
        let envelope = EnvelopeRef {
            version: 1,
            session,
        };
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)
            .map_err(|e| format!("Cannot create session snapshot: {e}"))?;
        struct Bounded<'a> {
            file: &'a mut File,
            written: u64,
        }
        impl Write for Bounded<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if bytes.len() as u64 > MAX_SNAPSHOT_BYTES.saturating_sub(self.written) {
                    return Err(std::io::Error::other("Session snapshot is too large"));
                }
                let count = self.file.write(bytes)?;
                self.written += count as u64;
                Ok(count)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.file.flush()
            }
        }
        serde_json::to_writer(
            Bounded {
                file: temporary.as_file_mut(),
                written: 0,
            },
            &envelope,
        )
        .map_err(|e| format!("Cannot write session snapshot: {e}"))?;
        temporary.flush().map_err(|e| e.to_string())?;
        if temporary
            .as_file()
            .metadata()
            .map_err(|e| e.to_string())?
            .len()
            > MAX_SNAPSHOT_BYTES
        {
            return Err("Session snapshot is too large.".into());
        }
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        temporary
            .persist(self.directory.join("session.json"))
            .map_err(|e| format!("Cannot save session snapshot: {e}"))?;
        #[cfg(unix)]
        File::open(&self.directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| format!("Session saved, but directory sync failed: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(workspace: &Path) -> Session {
        Session {
            workspace: workspace.canonicalize().unwrap(),
            tabs: vec![
                SessionTab {
                    path: Some(workspace.join("file.c")),
                    draft: "λ changed".into(),
                    base: Some("λ saved".into()),
                    anchor: 1,
                    head: 5,
                    dirty: true,
                },
                SessionTab {
                    path: None,
                    draft: "untitled draft".into(),
                    base: None,
                    anchor: 3,
                    head: 4,
                    dirty: true,
                },
            ],
            active: 1,
            explorer_visible: true,
            terminal_visible: false,
            panel_visible: true,
            panel: "output".into(),
            active_pane: "editor".into(),
        }
    }

    #[test]
    fn round_trip_keeps_drafts_base_and_selection() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::open_in(dir.path(), &dir.path().join("state")).unwrap();
        assert_eq!(store.load().unwrap(), None);
        let expected = session(dir.path());
        store.save(&expected).unwrap();
        assert_eq!(store.load().unwrap(), Some(expected));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(store.directory.join("session.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn concurrent_open_is_rejected_and_release_allows_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("state");
        let store = SessionStore::open_in(dir.path(), &base).unwrap();
        assert!(SessionStore::open_in(dir.path(), &base).is_err());
        drop(store);
        assert!(SessionStore::open_in(dir.path(), &base).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn paths_with_non_utf8_bytes_round_trip() {
        use std::os::unix::ffi::OsStringExt;
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::open_in(dir.path(), &dir.path().join("state")).unwrap();
        let mut value = session(dir.path());
        value.tabs[0].path = Some(
            dir.path()
                .join(std::ffi::OsString::from_vec(vec![b'x', 255])),
        );
        store.save(&value).unwrap();
        assert_eq!(store.load().unwrap(), Some(value));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_storage_is_rejected_without_chmod() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let alias = dir.path().join("alias");
        symlink(&target, &alias).unwrap();
        assert!(SessionStore::open_in(dir.path(), &alias).is_err());
        assert_eq!(
            fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn rejects_corrupt_and_foreign_sessions_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::open_in(dir.path(), &dir.path().join("state")).unwrap();
        let file = store.directory.join("session.json");
        fs::write(&file, b"broken").unwrap();
        assert!(store.load().is_err());
        let mut other = session(dir.path());
        other.workspace = "/different".into();
        assert!(store.save(&other).is_err());
        assert_eq!(fs::read(&file).unwrap(), b"broken");
    }
}
