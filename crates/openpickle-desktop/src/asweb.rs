//! Owned callback adapter for the main-queue Objective-C authentication bridge.
use std::ffi::{CStr, CString, c_char, c_int, c_void};

type Callback = Box<dyn FnOnce(Result<String, String>) + Send>;

unsafe extern "C" {
    fn openpickle_asweb_start(
        url: *const c_char,
        scheme: *const c_char,
        callback: extern "C" fn(*const c_char, *const c_char, *mut c_void),
        context: *mut c_void,
    ) -> c_int;
    fn openpickle_asweb_cancel();
}

extern "C" fn complete(url: *const c_char, error: *const c_char, context: *mut c_void) {
    // The bridge transfers this allocation exactly once, even on cancellation.
    let callback = unsafe { Box::from_raw(context.cast::<Callback>()) };
    let result = if !url.is_null() {
        Ok(unsafe { CStr::from_ptr(url) }
            .to_string_lossy()
            .into_owned())
    } else if !error.is_null() {
        Err(unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned())
    } else {
        Err("authentication failed".into())
    };
    // A Rust panic must never unwind through Objective-C.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(result)));
}

/// Start a session. A start error leaves no pending callback; accepted sessions
/// retain the callback until completion, replacement, or `cancel`.
pub fn start(url: &str, scheme: &str, callback: Callback) -> Result<(), String> {
    let expected = openpickle_api::kakao::KAKAO_REDIRECT_URI
        .split_once(':')
        .expect("Kakao redirect scheme")
        .0;
    if scheme != expected {
        return Err("unexpected callback scheme".into());
    }
    let url = CString::new(url).map_err(|_| "invalid authorization URL")?;
    let scheme = CString::new(scheme).map_err(|_| "invalid callback scheme")?;
    // Double boxing provides a thin, stable context pointer for the trait object.
    let context = Box::into_raw(Box::new(callback));
    let status =
        unsafe { openpickle_asweb_start(url.as_ptr(), scheme.as_ptr(), complete, context.cast()) };
    if status != 0 {
        // The bridge guarantees no callback on a nonzero return.
        unsafe {
            drop(Box::from_raw(context));
        }
        return Err("could not start ASWebAuthenticationSession".into());
    }
    Ok(())
}

pub fn cancel() {
    unsafe {
        openpickle_asweb_cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct Dropped(Arc<AtomicUsize>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn completion_copies_url_and_releases_callback() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let callback: Callback = Box::new(move |result| sender.send(result).unwrap());
        let context = Box::into_raw(Box::new(callback));
        let url = CString::new("test://callback?code=example").unwrap();
        complete(url.as_ptr(), std::ptr::null(), context.cast());
        drop(url);
        assert_eq!(
            receiver.recv().unwrap().unwrap(),
            "test://callback?code=example"
        );
    }

    #[test]
    fn cancellation_releases_callback_even_when_it_panics() {
        let count = Arc::new(AtomicUsize::new(0));
        let dropped = Dropped(count.clone());
        let callback: Callback = Box::new(move |result| {
            let _keep = dropped;
            assert_eq!(result, Err("cancelled".into()));
            panic!("callback panic must not cross FFI");
        });
        let context = Box::into_raw(Box::new(callback));
        complete(std::ptr::null(), c"cancelled".as_ptr(), context.cast());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn invalid_input_releases_callback_without_starting_session() {
        let count = Arc::new(AtomicUsize::new(0));
        let dropped = Dropped(count.clone());
        assert!(
            start(
                "https://example.com",
                "wrong-scheme",
                Box::new(move |_| {
                    let _keep = dropped;
                    panic!("rejected start must not call callback");
                })
            )
            .is_err()
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}
