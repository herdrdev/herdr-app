//! Pipe readers that can be stopped even when a descendant keeps the pipe open.

use std::io::{self, Read};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[cfg(unix)]
use std::os::fd::AsRawFd as PipeHandle;
#[cfg(windows)]
use std::os::windows::io::IntoRawHandle as PipeHandle;

pub(super) struct Reader {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<String>>,
    #[cfg(windows)]
    pipe: Arc<std::fs::File>,
}

impl Reader {
    pub(super) fn new(pipe: impl Read + PipeHandle + Send + 'static, limit: u64) -> Result<Self, String> {
        #[cfg(unix)]
        unsafe {
            let fd = pipe.as_raw_fd();
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags == -1 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == -1 {
                return Err(io::Error::last_os_error().to_string());
            }
        }
        #[cfg(windows)]
        let pipe = {
            use std::os::windows::io::FromRawHandle;
            // Keep this exact handle alive until cancellation and joining finish.
            Arc::new(unsafe { std::fs::File::from_raw_handle(pipe.into_raw_handle()) })
        };
        #[cfg(windows)]
        let cancel_pipe = pipe.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let thread = thread::spawn(move || {
            #[cfg(unix)]
            let mut pipe = pipe;
            #[cfg(windows)]
            let mut pipe = &*pipe;
            let mut output = Vec::new();
            let mut buffer = [0; 8192];
            while !cancelled.load(Ordering::Acquire) {
                match pipe.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        // Keep draining after the capture limit so a chatty child can exit.
                        let keep = n.min((limit as usize).saturating_sub(output.len()));
                        output.extend_from_slice(&buffer[..keep]);
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(10)),
                    Err(_) => break,
                }
            }
            String::from_utf8_lossy(&output).into_owned()
        });
        Ok(Self {
            stop, thread: Some(thread),
            #[cfg(windows)]
            pipe: cancel_pipe,
        })
    }

    pub(super) fn is_finished(&self) -> bool {
        self.thread.as_ref().unwrap().is_finished()
    }

    pub(super) fn finish(mut self) -> String {
        self.thread.take().unwrap().join().unwrap_or_default()
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let Some(thread) = self.thread.take() else { return };
        self.stop.store(true, Ordering::Release);
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            // A read can start just after the stop check. Retry cancellation until
            // the reader acknowledges shutdown, rather than racing a single cancel.
            while !thread.is_finished() {
                unsafe { windows_sys::Win32::System::IO::CancelIoEx(self.pipe.as_raw_handle(), std::ptr::null()); }
                thread::sleep(Duration::from_millis(10));
            }
        }
        let _ = thread.join();
    }
}
