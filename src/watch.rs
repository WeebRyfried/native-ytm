//! Watching the folders of music: Windows says when anything in them
//! changes (a song added, renamed, retagged or deleted), so they are looked
//! through again only then. The watching thread waits in the system until
//! it is told, using nothing meanwhile.

use std::path::PathBuf;
use std::sync::Arc;

/// Changes in a burst (a copy under way) are let settle this long before
/// the folders are looked through.
#[cfg(windows)]
const SETTLE_MS: u32 = 2000;

pub struct FolderWatch {
    #[cfg(windows)]
    stop: windows::Event,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FolderWatch {
    /// Watches `folders` (and everything under them) until dropped;
    /// `changed` runs once a burst of changes has settled.
    pub fn start(folders: &[String], changed: Arc<dyn Fn() + Send + Sync>) -> Option<Self> {
        let folders: Vec<PathBuf> = folders.iter().map(PathBuf::from).filter(|f| f.is_dir()).collect();
        if folders.is_empty() {
            return None;
        }
        start(folders, changed)
    }
}

impl Drop for FolderWatch {
    fn drop(&mut self) {
        #[cfg(windows)]
        self.stop.set();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(windows)]
fn start(folders: Vec<PathBuf>, changed: Arc<dyn Fn() + Send + Sync>) -> Option<FolderWatch> {
    let stop = windows::Event::new()?;
    let signal = stop.clone();
    let thread = std::thread::Builder::new()
        .name("folder-watch".into())
        .spawn(move || windows::watch(&folders, &signal, &*changed))
        .ok()?;
    Some(FolderWatch { stop, thread: Some(thread) })
}

#[cfg(not(windows))]
fn start(_folders: Vec<PathBuf>, _changed: Arc<dyn Fn() + Send + Sync>) -> Option<FolderWatch> {
    None
}

#[cfg(windows)]
mod windows {
    use std::os::windows::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::Arc;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};

    /// The most handles one wait takes, the stop event among them.
    const MOST: usize = 64;

    /// A manual-reset event, to wake the watching thread to stop.
    #[derive(Clone)]
    pub struct Event(Arc<Owned>);

    struct Owned(HANDLE);

    // SAFETY: an event handle may be used from any thread.
    unsafe impl Send for Owned {}
    unsafe impl Sync for Owned {}

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateEventW and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    impl Event {
        pub fn new() -> Option<Self> {
            // SAFETY: no security attributes or name; manual reset, unset.
            let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            (!handle.is_null()).then(|| Self(Arc::new(Owned(handle))))
        }

        pub fn set(&self) {
            // SAFETY: a live event handle.
            unsafe { SetEvent(self.0.0) };
        }

        fn handle(&self) -> HANDLE {
            self.0.0
        }
    }

    /// Waits on `folders` until `stop` is set, calling `changed` after
    /// each burst of changes.
    pub fn watch(folders: &[PathBuf], stop: &Event, changed: &(dyn Fn() + Send + Sync)) {
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE;
        let mut handles: Vec<HANDLE> = vec![stop.handle()];
        for folder in folders.iter().take(MOST - 1) {
            let wide: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
            // SAFETY: a NUL-terminated path; the whole tree is watched.
            let handle = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 1, filter) };
            if handle == INVALID_HANDLE_VALUE || handle.is_null() {
                log::info!("cannot watch {} for changes", folder.display());
            } else {
                handles.push(handle);
            }
        }
        if handles.len() > 1 {
            log::info!("watching {} folders of music for changes", handles.len() - 1);
            run(&handles, changed);
        }
        for &handle in &handles[1..] {
            // SAFETY: each came from FindFirstChangeNotificationW.
            unsafe { FindCloseChangeNotification(handle) };
        }
    }

    fn run(handles: &[HANDLE], changed: &(dyn Fn() + Send + Sync)) {
        let count = handles.len() as u32;
        loop {
            // SAFETY: live handles, waited on until one is signalled.
            let woke = unsafe { WaitForMultipleObjects(count, handles.as_ptr(), 0, INFINITE) };
            let Some(index) = woke.checked_sub(WAIT_OBJECT_0).map(|i| i as usize).filter(|i| *i < handles.len()) else {
                return;
            };
            if index == 0 {
                return;
            }
            rearm(handles[index]);
            // More may follow (a copy under way): wait for quiet.
            loop {
                // SAFETY: as above, with a timeout.
                let woke = unsafe { WaitForMultipleObjects(count, handles.as_ptr(), 0, super::SETTLE_MS) };
                if woke == WAIT_TIMEOUT {
                    break;
                }
                match woke.checked_sub(WAIT_OBJECT_0).map(|i| i as usize) {
                    Some(0) => return,
                    Some(i) if i < handles.len() => rearm(handles[i]),
                    _ => return,
                }
            }
            changed();
        }
    }

    fn rearm(handle: HANDLE) {
        // SAFETY: a live change notification handle.
        unsafe { FindNextChangeNotification(handle) };
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn says_when_a_folder_changes() {
        let dir = std::env::temp_dir().join(format!("native-ytm-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("album")).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let watch = FolderWatch::start(&[dir.to_string_lossy().into_owned()], Arc::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        }))
        .expect("watching");
        std::thread::sleep(std::time::Duration::from_millis(200));
        // A burst of changes deep in the tree makes one call, once settled.
        for n in 0..5 {
            std::fs::write(dir.join("album").join(format!("{n}.mp3")), b"x").unwrap();
        }
        let mut waited = 0;
        while count.load(Ordering::SeqCst) == 0 && waited < 60 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            waited += 1;
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        drop(watch);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
