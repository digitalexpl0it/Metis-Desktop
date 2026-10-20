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
            // Keep atlas dimensions in the header even when capacity fits.
            if self.current_width() != width || self.current_height() != height {
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
            }
            return Ok(());
        }
        *self = Self::create(&self.path.clone(), width, height)?;
        Ok(())
    }

    /// Publish a packed BGRX/BGRx frame covering the whole atlas (single-monitor).
    #[allow(dead_code)]
    pub fn publish_bgrx(
        &mut self,
        width: u32,
        height: u32,
        src_stride: u32,
        pixels: &[u8],
    ) -> io::Result<()> {
        self.ensure_size(width, height)?;
        self.blit_region(0, 0, width, height, src_stride, pixels)
    }

    /// Blit a monitor tile into the atlas at `(dst_x, dst_y)` (atlas-relative).
    pub fn blit_region(
        &mut self,
        dst_x: i32,
        dst_y: i32,
        width: u32,
        height: u32,
        src_stride: u32,
        pixels: &[u8],
    ) -> io::Result<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        let atlas_w = self.current_width().max(1);
        let atlas_h = self.current_height().max(1);
        let stride = atlas_w.saturating_mul(4);
        let needed = (src_stride as usize).saturating_mul(height as usize);
        let tight = (width as usize)
            .saturating_mul(4)
            .saturating_mul(height as usize);
        if pixels.len() < needed && pixels.len() < tight {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "frame buffer too small for source pixels",
            ));
        }

        let dst_x = dst_x.max(0) as u32;
        let dst_y = dst_y.max(0) as u32;
        if dst_x >= atlas_w || dst_y >= atlas_h {
            return Ok(());
        }
        let copy_w = width.min(atlas_w - dst_x);
        let copy_h = height.min(atlas_h - dst_y);
        let row_bytes = (copy_w as usize).saturating_mul(4);

        self.seq = self.seq.wrapping_add(1);
        let hdr = FrameHeader {
            magic: FRAME_MAGIC,
            width: atlas_w,
            height: atlas_h,
            stride,
            seq: self.seq,
            ready: 0,
            reserved: [0; 2],
        };
        unsafe {
            std::ptr::write_unaligned(self.map.as_mut_ptr().cast::<FrameHeader>(), hdr);
        }
        let dst_base = std::mem::size_of::<FrameHeader>();
        for row in 0..copy_h as usize {
            let src_off = row * src_stride as usize;
            let dst_off =
                dst_base + ((dst_y as usize + row) * stride as usize) + (dst_x as usize) * 4;
            if src_off + row_bytes > pixels.len() || dst_off + row_bytes > self.map.len() {
                break;
            }
            self.map[dst_off..dst_off + row_bytes]
                .copy_from_slice(&pixels[src_off..src_off + row_bytes]);
        }
        let mut hdr = hdr;
        hdr.ready = 1;
        unsafe {
            std::ptr::write_unaligned(self.map.as_mut_ptr().cast::<FrameHeader>(), hdr);
        }
        let _ = self.map.flush();
        Ok(())
    }

    fn current_width(&self) -> u32 {
        if self.map.len() < std::mem::size_of::<FrameHeader>() {
            return 0;
        }
        unsafe { std::ptr::read_unaligned(self.map.as_ptr().cast::<FrameHeader>()).width }
    }

    fn current_height(&self) -> u32 {
        if self.map.len() < std::mem::size_of::<FrameHeader>() {
            return 0;
        }
        unsafe { std::ptr::read_unaligned(self.map.as_ptr().cast::<FrameHeader>()).height }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_shm() -> PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("metis-rdp-fb-test-{n}.shm"))
    }

    #[test]
    fn blit_region_into_atlas() {
        let path = temp_shm();
        let mut fb = FrameBuffer::create(&path, 200, 100).expect("create");
        let tile = vec![0x11u8; 100 * 50 * 4];
        fb.blit_region(50, 25, 100, 50, 400, &tile).expect("blit");
        let hdr = unsafe { std::ptr::read_unaligned(fb.map.as_ptr().cast::<FrameHeader>()) };
        assert_eq!(hdr.width, 200);
        assert_eq!(hdr.height, 100);
        assert_eq!(hdr.ready, 1);
        let off = std::mem::size_of::<FrameHeader>() + (25 * 200 * 4) + (50 * 4);
        assert_eq!(fb.map[off], 0x11);
        let _ = std::fs::remove_file(&path);
    }
}
