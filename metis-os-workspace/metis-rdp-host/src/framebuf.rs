//! Shared-memory frame buffer consumed by the FreeRDP shadow subsystem.

use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

use memmap2::{MmapMut, MmapOptions};

pub const FRAME_MAGIC: u32 = 0x4D46_524D; // 'MFRM'

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FrameHeader {
    pub magic: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub seq: u32,
    pub ready: u32,
    pub reserved: [u32; 2],
}

pub struct FrameBuffer {
    path: PathBuf,
    map: MmapMut,
    capacity: usize,
    seq: u32,
}

impl FrameBuffer {
    pub fn create(path: &Path, width: u32, height: u32) -> io::Result<Self> {
        let stride = width.saturating_mul(4);
        let capacity = std::mem::size_of::<FrameHeader>() + (stride as usize) * (height as usize);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len(capacity as u64)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        let mut map = unsafe { MmapOptions::new().len(capacity).map_mut(&file)? };
        let hdr = FrameHeader {
            magic: FRAME_MAGIC,
            width,
            height,
            stride,
            seq: 0,
            ready: 0,
            reserved: [0; 2],
        };
        // SAFETY: header is POD at the start of the mapping.
        unsafe {
            std::ptr::write_unaligned(map.as_mut_ptr().cast::<FrameHeader>(), hdr);
        }
        Ok(Self {
            path: path.to_path_buf(),
            map,
            capacity,
            seq: 0,
        })
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn ensure_size(&mut self, width: u32, height: u32) -> io::Result<()> {
        let stride = width.saturating_mul(4);
        let need = std::mem::size_of::<FrameHeader>() + (stride as usize) * (height as usize);
        if need <= self.capacity {
            return Ok(());
        }
        *self = Self::create(&self.path.clone(), width, height)?;
        Ok(())
    }

    /// Publish a packed BGRX/BGRx frame (`width * height * 4` tightly packed OK;
    /// `src_stride` may be larger).
    pub fn publish_bgrx(
        &mut self,
        width: u32,
        height: u32,
        src_stride: u32,
        pixels: &[u8],
    ) -> io::Result<()> {
        self.ensure_size(width, height)?;
        let stride = width.saturating_mul(4);
        let needed = (src_stride as usize).saturating_mul(height as usize);
        if pixels.len() < needed && pixels.len() < (stride as usize) * (height as usize) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "frame buffer too small for source pixels",
            ));
        }
        self.seq = self.seq.wrapping_add(1);
        let hdr = FrameHeader {
            magic: FRAME_MAGIC,
            width,
            height,
            stride,
            seq: self.seq,
            ready: 0,
            reserved: [0; 2],
        };
        unsafe {
            std::ptr::write_unaligned(self.map.as_mut_ptr().cast::<FrameHeader>(), hdr);
        }
        let dst_base = std::mem::size_of::<FrameHeader>();
        for row in 0..height as usize {
            let src_off = row * src_stride as usize;
            let dst_off = dst_base + row * stride as usize;
            let n = stride as usize;
            if src_off + n > pixels.len() || dst_off + n > self.map.len() {
                break;
            }
            self.map[dst_off..dst_off + n].copy_from_slice(&pixels[src_off..src_off + n]);
        }
        let mut hdr = hdr;
        hdr.ready = 1;
        unsafe {
            std::ptr::write_unaligned(self.map.as_mut_ptr().cast::<FrameHeader>(), hdr);
        }
        let _ = self.map.flush();
        Ok(())
    }
}
