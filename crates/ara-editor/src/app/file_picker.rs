//! One native picker owns one callback context and returns one complete file batch.
use ara_platform::Window;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::{Path, PathBuf},
    sync::mpsc::Sender,
};

/// Native picker completion; its destination is the folder captured at opening.
#[derive(Debug)]
pub(super) enum Selection {
    /// A complete selected batch, still awaiting authored-scope resolution/import.
    Files {
        sources: Vec<PathBuf>,
        destination: String,
    },
    /// The user closed the picker without selecting files.
    Cancelled,
    /// SDL or UTF-8 conversion failed; no partial batch was sent.
    Failed(String),
}

struct Context {
    destination: String,
    sender: Sender<Selection>,
    // Retain SDL's default-location bytes until its asynchronous completion.
    location: CString,
}

/// Open on the window-owning thread. The app blocks this call when offscreen.
/// The callback does no IO and retains no Editor, Shell or OS-window borrow.
pub(super) fn show(
    window: &Window,
    root: &Path,
    storage_folder: String,
    sender: Sender<Selection>,
) {
    let location = match default_location(root, &storage_folder) {
        Ok(location) => location,
        Err(error) => {
            let _ = sender.send(Selection::Failed(error));
            return;
        }
    };
    let context = Box::new(Context {
        destination: storage_folder,
        sender,
        location,
    });
    let location = context.location.as_ptr();
    let userdata = Box::into_raw(context).cast::<c_void>();
    // SAFETY: The caller owns a live Window on this thread. SDL invokes chosen
    // exactly once, including cancellation/error (SDL_dialog.h), transferring
    // this Box back to it. Its CString stays live through that callback. Filters
    // are null. Do not access userdata after the call: SDL may complete inline.
    unsafe {
        sdl3_sys::dialog::SDL_ShowOpenFileDialog(
            Some(chosen),
            userdata,
            window.raw(),
            std::ptr::null(),
            0,
            location,
            true,
        );
    }
}

fn default_location(root: &Path, storage_folder: &str) -> Result<CString, String> {
    let path = root.join(storage_folder);
    let path = path
        .to_str()
        .ok_or("The current asset folder cannot be represented as UTF-8")?;
    CString::new(path).map_err(|_| "The current asset folder contains a NUL character".into())
}

unsafe extern "C" fn chosen(userdata: *mut c_void, files: *const *const c_char, _filter: c_int) {
    // SAFETY: show supplies one Box pointer, and SDL returns it to this one
    // completion callback. Taking ownership here drops it on every return path,
    // even cancellation, invalid UTF-8, SDL error or a disconnected receiver.
    let context = unsafe { Box::from_raw(userdata.cast::<Context>()) };
    let result = if files.is_null() {
        // SAFETY: SDL_GetError returns a thread-local NUL-terminated string.
        // SDL requires reading the error inside this callback's thread.
        let error = unsafe { CStr::from_ptr(sdl3_sys::error::SDL_GetError()) }.to_string_lossy();
        Selection::Failed(if error.is_empty() {
            "The native import picker could not open".into()
        } else {
            format!("The native import picker failed: {error}")
        })
    } else {
        // SAFETY: SDL owns a NUL-terminated pointer array of NUL-terminated paths,
        // valid until this callback returns; copy every path before sending.
        match unsafe { copy_paths(files) } {
            Ok(sources) if sources.is_empty() => Selection::Cancelled,
            Ok(sources) => Selection::Files {
                sources,
                destination: context.destination.clone(),
            },
            Err(error) => Selection::Failed(error),
        }
    };
    let _ = context.sender.send(result);
}

/// Copy SDL-owned paths without replacement characters or filesystem access.
unsafe fn copy_paths(mut files: *const *const c_char) -> Result<Vec<PathBuf>, String> {
    let mut sources = Vec::new();
    loop {
        // SAFETY: The caller guarantees SDL's live NUL-terminated pointer array.
        let path = unsafe { *files };
        if path.is_null() {
            return Ok(sources);
        }
        // SAFETY: Every non-null array element is a live NUL-terminated C string.
        let path = unsafe { CStr::from_ptr(path) }
            .to_str()
            .map_err(|_| "A selected import path is not valid UTF-8".to_owned())?;
        sources.push(PathBuf::from(path));
        // SAFETY: The pointer array includes the next element or its terminator.
        files = unsafe { files.add(1) };
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, TryRecvError};

    fn context(destination: &str) -> (*mut c_void, mpsc::Receiver<Selection>) {
        let (sender, receiver) = mpsc::channel();
        let context = Box::new(Context {
            destination: destination.into(),
            sender,
            location: CString::new("unused-test-location").unwrap(),
        });
        (Box::into_raw(context).cast(), receiver)
    }

    #[test]
    fn one_callback_returns_the_whole_batch_with_its_original_destination() {
        let first = CString::new("/external/first.wav").unwrap();
        let second = CString::new("/external/second.wav").unwrap();
        let paths = [first.as_ptr(), second.as_ptr(), std::ptr::null()];
        let (context, receiver) = context("game/Original folder");
        // SAFETY: This fresh Context is consumed once; local C strings and their
        // terminated pointer array stay live until chosen returns.
        unsafe { chosen(context, paths.as_ptr(), -1) };
        let Selection::Files {
            sources,
            destination,
        } = receiver.recv().unwrap()
        else {
            panic!("Expected one complete selected batch")
        };
        assert_eq!(
            sources,
            [
                PathBuf::from("/external/first.wav"),
                PathBuf::from("/external/second.wav")
            ]
        );
        assert_eq!(destination, "game/Original folder");
        assert!(
            matches!(receiver.try_recv(), Err(TryRecvError::Disconnected)),
            "callback context and its only Sender must be released after one batch"
        );
    }

    #[test]
    fn cancellation_error_and_invalid_utf8_release_the_owned_request() {
        let valid = CString::new("/external/valid-before-invalid.wav").unwrap();
        let invalid = CString::new(vec![0xff]).unwrap();
        let cancelled = [std::ptr::null()];
        let invalid_paths = [valid.as_ptr(), invalid.as_ptr(), std::ptr::null()];
        for (files, cancellation) in [
            (cancelled.as_ptr(), true),
            (std::ptr::null(), false),
            (invalid_paths.as_ptr(), false),
        ] {
            let (context, receiver) = context("game/Original folder");
            // SAFETY: One newly allocated Context per callback; each non-null
            // pointer array is terminated and its C strings live through the call.
            unsafe { chosen(context, files, -1) };
            match receiver.recv().unwrap() {
                Selection::Cancelled => assert!(cancellation),
                Selection::Failed(message) => assert!(!cancellation && !message.is_empty()),
                Selection::Files { .. } => {
                    panic!("An invalid selection must not send a partial batch")
                }
            }
            assert!(matches!(
                receiver.try_recv(),
                Err(TryRecvError::Disconnected)
            ));
        }
    }

    #[test]
    fn invalid_default_location_is_reportable_before_showing_any_dialog() {
        assert!(default_location(Path::new("project"), "bad\0folder").is_err());
    }
}
