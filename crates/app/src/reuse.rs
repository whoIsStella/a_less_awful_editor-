use std::{
    ffi::OsString,
    hash::{Hash, Hasher},
    io::{Read, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    time::Duration,
};

type Request = (Vec<PathBuf>, usize, usize);

fn socket_path() -> Result<PathBuf, String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
    let dir = PathBuf::from(dir);
    let meta = std::fs::symlink_metadata(&dir).map_err(|e| e.to_string())?;
    if !meta.is_dir()
        || meta.permissions().mode() & 0o077 != 0
        || meta.uid() != unsafe { libc::geteuid() }
    {
        return Err("XDG_RUNTIME_DIR must be an owner-only directory owned by this user".into());
    }
    let display = std::env::var("WAYLAND_DISPLAY")
        .or_else(|_| std::env::var("DISPLAY"))
        .unwrap_or_else(|_| "default".into());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    display.hash(&mut hasher);
    Ok(dir.join(format!("ale-editor-{:016x}", hasher.finish())))
}

fn read_number(stream: &mut UnixStream) -> std::io::Result<u32> {
    let mut bytes = [0; 4];
    stream.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

fn decode(stream: &mut UnixStream) -> std::io::Result<Request> {
    let line = read_number(stream)? as usize;
    let column = read_number(stream)? as usize;
    let count = read_number(stream)? as usize;
    if line == 0 || column == 0 || count == 0 || count > 32 {
        return Err(std::io::Error::other("invalid open request"));
    }
    let mut paths = Vec::with_capacity(count);
    for _ in 0..count {
        let len = read_number(stream)? as usize;
        if len == 0 || len > 32768 {
            return Err(std::io::Error::other("invalid path"));
        }
        let mut bytes = vec![0; len];
        stream.read_exact(&mut bytes)?;
        let path = PathBuf::from(OsString::from_vec(bytes));
        if !path.is_absolute() {
            return Err(std::io::Error::other("path must be absolute"));
        }
        paths.push(path);
    }
    Ok((paths, line, column))
}

pub struct Server {
    path: PathBuf,
    inode: u64,
}
impl Drop for Server {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|m| m.file_type().is_socket() && m.ino() == self.inode)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub fn start() -> Result<(Server, Receiver<Request>), String> {
    let prefix = socket_path()?;
    let mut name = prefix.into_os_string();
    name.push(format!("-{}.sock", std::process::id()));
    let path = PathBuf::from(name);
    let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
    let inode = std::fs::symlink_metadata(&path)
        .map_err(|e| e.to_string())?
        .ino();
    let (sender, receiver) = mpsc::sync_channel(32);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let result = decode(&mut stream).and_then(|request| {
                sender
                    .try_send(request)
                    .map_err(|e| std::io::Error::other(e.to_string()))
            });
            let _ = stream.write_all(if result.is_ok() { b"OK" } else { b"NO" });
        }
    });
    Ok((Server { path, inode }, receiver))
}

pub fn send(paths: Vec<PathBuf>, line: usize, column: usize) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    if paths.is_empty()
        || paths.len() > 32
        || line == 0
        || column == 0
        || line > u32::MAX as usize
        || column > u32::MAX as usize
    {
        return Err("Open requests require 1 to 32 paths and valid line and column.".into());
    }
    let prefix = socket_path()?;
    let parent = prefix.parent().ok_or("Invalid runtime directory")?;
    let file_prefix = prefix
        .file_name()
        .ok_or("Invalid socket prefix")?
        .to_string_lossy();
    let mut candidates = std::fs::read_dir(parent)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(&*file_prefix))
        })
        .collect::<Vec<_>>();
    candidates.sort();
    let mut stream = candidates
        .iter()
        .find_map(|path| UnixStream::connect(path).ok())
        .ok_or("No available editor window in this desktop session.")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    for n in [line as u32, column as u32, paths.len() as u32] {
        stream
            .write_all(&n.to_be_bytes())
            .map_err(|e| e.to_string())?;
    }
    for path in paths {
        let path = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        let bytes = path.as_os_str().as_bytes();
        if bytes.is_empty() || bytes.len() > 32768 {
            return Err("Invalid path length".into());
        }
        stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .and_then(|_| stream.write_all(bytes))
            .map_err(|e| e.to_string())?;
    }
    let mut ack = [0; 2];
    stream.read_exact(&mut ack).map_err(|e| e.to_string())?;
    if &ack == b"OK" {
        Ok(())
    } else {
        Err("Editor could not accept the open request.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_preserves_non_unicode_path_and_rejects_relative_path() {
        let (mut writer, mut reader) = UnixStream::pair().unwrap();
        let path = b"/tmp/file-\xff.c";
        for n in [4_u32, 3, 1, path.len() as u32] {
            writer.write_all(&n.to_be_bytes()).unwrap();
        }
        writer.write_all(path).unwrap();
        let (paths, line, column) = decode(&mut reader).unwrap();
        assert_eq!(paths[0].as_os_str().as_bytes(), path);
        assert_eq!((line, column), (4, 3));

        let (mut writer, mut reader) = UnixStream::pair().unwrap();
        for n in [1_u32, 1, 1, 3] {
            writer.write_all(&n.to_be_bytes()).unwrap();
        }
        writer.write_all(b"rel").unwrap();
        assert!(decode(&mut reader).is_err());
    }
}
