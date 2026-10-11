//! Whether the internet is there, as Windows knows it (the Network List
//! Manager, which also tells a network without internet from one with).
//!
//! Asked when a request fails in a way that may mean the connection went,
//! and then every few seconds until it is back. While all is well nothing
//! runs.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

/// How often to look again while offline.
const RECHECK: Duration = Duration::from_secs(3);

pub struct Network {
    online: Arc<AtomicBool>,
    kick: Arc<(Mutex<bool>, Condvar)>,
}

impl Network {
    /// Starts watching; `wake` runs whenever the answer changes.
    pub fn start(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let online = Arc::new(AtomicBool::new(true));
        let kick = Arc::new((Mutex::new(true), Condvar::new()));
        let (flag, signal) = (online.clone(), kick.clone());
        let spawned = std::thread::Builder::new().name("network".into()).spawn(move || {
            com_init();
            loop {
                let now = internet().unwrap_or(true);
                if flag.swap(now, Ordering::SeqCst) != now {
                    log::info!("{}", if now { "back online" } else { "offline" });
                    wake();
                }
                let (lock, condvar) = &*signal;
                let mut kicked = lock.lock();
                if !*kicked {
                    if now {
                        condvar.wait(&mut kicked);
                    } else {
                        condvar.wait_for(&mut kicked, RECHECK);
                    }
                }
                *kicked = false;
            }
        });
        if let Err(error) = spawned {
            log::warn!("no network watch: {error}");
        }
        Self { online, kick }
    }

    pub fn online(&self) -> bool {
        self.online.load(Ordering::SeqCst)
    }

    /// Looks now, on this thread, and has the watch look again until the
    /// connection is back. For a request that just failed.
    pub fn check_now(&self) -> bool {
        com_init();
        let now = internet().unwrap_or(true);
        self.online.store(now, Ordering::SeqCst);
        let (lock, condvar) = &*self.kick;
        *lock.lock() = true;
        condvar.notify_one();
        now
    }
}

/// Whether an error from a request says the network could not be reached,
/// rather than that the server answered no.
pub fn unreachable(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    ["error sending request", "dns", "connect", "timed out", "unreachable", "no route", "network"]
        .iter()
        .any(|word| error.contains(word))
}

#[cfg(windows)]
fn com_init() {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    // SAFETY: initialising COM for this thread; a thread already set up
    // (the window's, as single-threaded) just says so, which is fine.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

#[cfg(not(windows))]
fn com_init() {}

/// Whether Windows sees internet on any network; None when it cannot say.
/// `NATIVE_YTM_OFFLINE` says no, to try the offline paths (dev).
#[cfg(windows)]
fn internet() -> Option<bool> {
    if std::env::var_os("NATIVE_YTM_OFFLINE").is_some() {
        return Some(false);
    }
    use windows::Win32::Networking::NetworkListManager::{
        INetworkListManager, NLM_CONNECTIVITY_IPV4_INTERNET, NLM_CONNECTIVITY_IPV6_INTERNET, NetworkListManager,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    // SAFETY: plain COM calls on a thread with COM initialised.
    unsafe {
        let manager: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok()?;
        let connectivity = manager.GetConnectivity().ok()?;
        Some(connectivity.0 & (NLM_CONNECTIVITY_IPV4_INTERNET.0 | NLM_CONNECTIVITY_IPV6_INTERNET.0) != 0)
    }
}

#[cfg(not(windows))]
fn internet() -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_network_errors_from_refusals() {
        assert!(unreachable("browse request: error sending request"));
        assert!(unreachable("dns error: no such host is known"));
        assert!(!unreachable("browse: HTTP 403 Forbidden: The caller does not have permission"));
    }

    #[test]
    #[cfg(windows)]
    fn windows_answers() {
        com_init();
        assert!(internet().is_some());
    }
}
