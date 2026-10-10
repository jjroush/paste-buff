//! `SIGUSR1` toggles the window. This lets the benchmark harness exercise a
//! frontend without the Accessibility permission that synthetic keystrokes
//! need.
//!
//! The handler writes one byte to a self-pipe; a dedicated thread blocks in
//! `read` and runs the callback once per byte, so nothing polls.

use std::io;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;

static WRITE_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(_: libc::c_int) {
    // Only async-signal-safe calls belong here.
    let fd = WRITE_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        unsafe { libc::write(fd, b"x".as_ptr() as *const libc::c_void, 1) };
    }
}

/// Install the handler and call `on_toggle` (from a background thread) once
/// per received signal.
pub fn listen(on_toggle: impl Fn() + Send + 'static) -> io::Result<()> {
    let mut fds = [-1 as libc::c_int; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let [read_fd, write_fd] = fds;
    unsafe {
        libc::fcntl(read_fd, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(write_fd, libc::F_SETFD, libc::FD_CLOEXEC);
        // A full pipe must never block the signal handler.
        libc::fcntl(write_fd, libc::F_SETFL, libc::O_NONBLOCK);
    }
    WRITE_FD.store(write_fd, Ordering::SeqCst);
    let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    unsafe {
        libc::signal(libc::SIGUSR1, handler);
    }
    thread::Builder::new()
        .name("signal-listener".into())
        .spawn(move || {
            let mut buf = [0u8; 16];
            loop {
                let n = unsafe {
                    libc::read(read_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
                };
                if n > 0 {
                    for _ in 0..n {
                        on_toggle();
                    }
                } else if n < 0
                    && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
                {
                    continue;
                } else {
                    break;
                }
            }
        })?;
    Ok(())
}
