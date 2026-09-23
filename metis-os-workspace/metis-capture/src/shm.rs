//! SHM buffer for ext-image-copy-capture clients.

use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};

use wayland_client::{
    Dispatch, QueueHandle,
    protocol::{wl_buffer::WlBuffer, wl_shm::Format, wl_shm::WlShm, wl_shm_pool::WlShmPool},
};

#[derive(Debug, Clone, Copy)]
pub struct BufferFormat {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

pub struct ShmBuffer {
    pub buffer: WlBuffer,
    _pool: WlShmPool,
    mmap: *mut u8,
    pub size: usize,
    pub format: BufferFormat,
}

// SAFETY: used only on the capture thread.
unsafe impl Send for ShmBuffer {}

impl ShmBuffer {
    pub fn new<D>(shm: &WlShm, qh: &QueueHandle<D>, format: BufferFormat) -> Result<Self, String>
    where
        D: Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, ()> + 'static,
    {
        let size = (format.stride * format.height) as usize;
        if size == 0 {
            return Err("zero-sized capture buffer".into());
        }

        let fd = memfd(size)?;
        let mmap = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_fd().as_raw_fd(),
                0,
            )
        };
        if mmap == libc::MAP_FAILED {
            return Err(format!("mmap failed: {}", std::io::Error::last_os_error()));
        }

        let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            format.width as i32,
            format.height as i32,
            format.stride as i32,
            format.format,
            qh,
            (),
        );

        Ok(Self {
            buffer,
            _pool: pool,
            mmap: mmap as *mut u8,
            size,
            format,
        })
    }

    pub fn pixels(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.mmap, self.size) }
    }
}

fn memfd(size: usize) -> Result<OwnedFd, String> {
    let name = b"metis-capture\0";
    let fd = unsafe { libc::memfd_create(name.as_ptr() as *const libc::c_char, 0) };
    if fd < 0 {
        return Err(format!(
            "memfd_create failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if unsafe { libc::ftruncate(fd.as_fd().as_raw_fd(), size as libc::off_t) } != 0 {
        return Err(format!(
            "ftruncate failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(fd)
}
