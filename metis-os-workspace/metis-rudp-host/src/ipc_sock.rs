//! Unix-socket plumbing for the compositor ↔ `metis-rudp-host` link:
//! `0700` runtime dir, `0600` socket, `SO_PEERCRED` UID verification.

use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

/// Create (or validate) the private IPC directory and return the socket path
/// inside it. The directory must be owned by us and not group/world accessible.
pub fn prepare_ipc_dir() -> Result<PathBuf, String> {
    let dir = metis_protocol::rudp_host_ipc_dir();
    match fs::metadata(&dir) {
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(format!("{} is not a directory", dir.display()));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)
                .map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
        Err(e) => return Err(format!("stat {}: {e}", dir.display())),
    }
    let meta = fs::metadata(&dir).map_err(|e| format!("stat {}: {e}", dir.display()))?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let euid = unsafe { libc::geteuid() };
    if meta.uid() != euid {
        return Err(format!(
            "{} is owned by uid {} (expected {euid})",
            dir.display(),
            meta.uid()
        ));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("chmod 0700 {}: {e}", dir.display()))?;
    }
    Ok(dir.join(metis_protocol::RUDP_HOST_IPC_SOCK))
}

/// Bind the listening socket at `path` (replacing a stale socket file) with
/// mode `0600`.
pub fn bind_listener(path: &Path) -> Result<UnixListener, String> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove stale {}: {e}", path.display())),
    }
    let listener = UnixListener::bind(path).map_err(|e| format!("bind {}: {e}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod 0600 {}: {e}", path.display()))?;
    Ok(listener)
}

/// UID of the process on the other end of `stream` (`SO_PEERCRED`).
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred`/`len` are valid for writes of the sizes passed, and the fd
    // is a live socket owned by `stream` for the duration of the call.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred.uid)
}

/// Reject any peer that is not running as our effective UID.
pub fn verify_peer(stream: &UnixStream) -> Result<(), String> {
    let uid = peer_uid(stream).map_err(|e| format!("SO_PEERCRED: {e}"))?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let euid = unsafe { libc::geteuid() };
    if uid != euid {
        return Err(format!("peer uid {uid} != {euid}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socketpair_peer_is_us() {
        let (a, b) = UnixStream::pair().expect("pair");
        assert!(verify_peer(&a).is_ok());
        assert!(verify_peer(&b).is_ok());
    }
}
