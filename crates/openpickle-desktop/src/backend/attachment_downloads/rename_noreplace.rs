//! Same-volume atomic publication for filesystems that reject hard links, such as exFAT.
//! If the platform or volume does not support no-replace rename, fail rather than copying,
//! creating a placeholder, or checking for existence before an ordinary rename.

use std::{io, path::Path};

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn rename(from: &Path, to: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    // renameat2(RENAME_NOREPLACE) on Linux, renameatx_np(RENAME_EXCL) on macOS.
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(Into::into)
}

#[cfg(windows)]
pub(super) fn rename(from: &Path, to: &Path) -> io::Result<()> {
    use std::{fs, os::windows::ffi::OsStrExt as _};
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
        let mut wide: Vec<_> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        wide.push(0);
        Ok(wide)
    }

    // Both names are in the same folder. Canonicalizing it gives us a verbatim path,
    // including for folders and filenames that need Windows extended-length paths.
    let folder = fs::canonicalize(from.parent().expect("staging folder"))?;
    let from = wide_path(&folder.join(from.file_name().expect("staging filename")))?;
    let to = wide_path(&folder.join(to.file_name().expect("download filename")))?;
    // SAFETY: the live buffers hold NUL-terminated UTF-16 paths. Zero flags forbid
    // replacement and cross-volume copy, leaving only the same-volume rename.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(super) fn rename(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "This platform requires hard links to save downloads without replacing files.",
    ))
}
