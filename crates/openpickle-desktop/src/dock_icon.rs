//! The running app's Dock and app-switcher icon on macOS.
//!
//! The bundled `.icns` is left alone: each installation keeps the upstream mark,
//! and the custom picture is applied only while this process is signed in. Other
//! platforms are a no-op.

use std::path::Path;

/// Shows `path` as the Dock icon, or restores the bundled icon when `path` is
/// `None` or cannot be composed.
pub fn apply(path: Option<&Path>) {
    #[cfg(target_os = "macos")]
    macos::apply(path);
    #[cfg(not(target_os = "macos"))]
    let _ = path;
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{
        ffi::{CString, c_char, c_int},
        path::Path,
    };

    unsafe extern "C" {
        fn openpickle_dock_icon_set(path: *const c_char) -> c_int;
        fn openpickle_dock_icon_reset();
        #[cfg(test)]
        fn openpickle_dock_icon_render(src: *const c_char, dest: *const c_char) -> c_int;
    }

    pub(super) fn apply(path: Option<&Path>) {
        match path.and_then(to_cstring) {
            Some(path) if unsafe { openpickle_dock_icon_set(path.as_ptr()) } == 0 => {}
            _ => unsafe { openpickle_dock_icon_reset() },
        }
    }

    #[cfg(test)]
    pub(super) fn render(src: &Path, dest: &Path) -> Result<(), i32> {
        let src = to_cstring(src).ok_or(-1)?;
        let dest = to_cstring(dest).ok_or(-1)?;
        match unsafe { openpickle_dock_icon_render(src.as_ptr(), dest.as_ptr()) } {
            0 => Ok(()),
            status => Err(status),
        }
    }

    fn to_cstring(path: &Path) -> Option<CString> {
        CString::new(path.as_os_str().as_encoded_bytes()).ok()
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::macos::render;
    use std::{fs, io::Write, path::PathBuf};

    /// A 2×2 PNG, red on the top row and blue on the bottom.
    const SOURCE_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72,
        0xb6, 0x0d, 0x24, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x84, 0x19, 0xa0, 0xf4, 0x7f, 0x00, 0x43, 0xce, 0x07, 0xf9, 0x00,
        0xf3, 0x98, 0x01, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn unique_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "openpickle-dock-icon-{}-{name}",
            uuid::Uuid::new_v4()
        ))
    }

    fn write_source() -> PathBuf {
        let path = unique_path("src.png");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(SOURCE_PNG).unwrap();
        path
    }

    fn png_size(bytes: &[u8]) -> (u32, u32) {
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        (width, height)
    }

    #[test]
    fn render_writes_a_2048_png() {
        let src = write_source();
        let dest = unique_path("out.png");
        render(&src, &dest).unwrap();
        let bytes = fs::read(&dest).unwrap();
        assert_eq!(png_size(&bytes), (2048, 2048));
        fs::remove_file(src).ok();
        fs::remove_file(dest).ok();
    }

    #[test]
    fn render_rejects_a_missing_source() {
        let dest = unique_path("missing.png");
        assert!(
            render(
                PathBuf::from("/no/such/openpickle-avatar.png").as_path(),
                &dest
            )
            .is_err()
        );
        assert!(!dest.exists());
    }
}
