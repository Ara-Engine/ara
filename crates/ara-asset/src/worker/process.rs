//! Child lifetime and admission are independent of consuming its protocol reply.
use super::{background_priority, limit::Permit};
use std::{
    process::Child,
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};

pub(super) struct Running {
    #[cfg(feature = "profiling")]
    id: u32,
    cancellation: Sender<()>,
}

impl Running {
    pub(super) fn new(mut child: Child, permit: Permit) -> Self {
        let (cancellation, requests) = mpsc::channel();
        let running = Self {
            #[cfg(feature = "profiling")]
            id: child.id(),
            cancellation,
        };
        thread::spawn(move || {
            background_priority();
            // Reap even when the owner leaves a completed reply unpolled. Keep
            // admission until exit is confirmed, including kill/wait failures.
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {}
                    Err(_) => {
                        let _ = child.kill();
                    }
                }
                match requests.recv_timeout(Duration::from_millis(2)) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                        let _ = child.kill();
                        while child.wait().is_err() {
                            thread::sleep(Duration::from_millis(2));
                        }
                        break;
                    }
                }
            }
            drop(permit);
        });
        running
    }

    #[cfg(feature = "profiling")]
    pub(super) fn id(&self) -> u32 {
        self.id
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.cancellation.send(());
    }
}
