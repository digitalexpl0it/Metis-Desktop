//! GBM + linux-dmabuf capture buffers for zero-copy ScreenCast.

use std::fs::OpenOptions;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use drm_fourcc::{DrmFourcc, DrmModifier};
use gbm::{BufferObject, BufferObjectFlags, Device as GbmDevice, Format as GbmFormat};
use wayland_client::{protocol::wl_buffer::WlBuffer, Dispatch, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
};

/// Plane descriptors exported for PipeWire `SPA_DATA_DmaBuf`.
#[derive(Debug)]
pub struct DmabufPlanes {
    pub fds: Vec<OwnedFd>,
    pub offsets: Vec<u32>,
    pub strides: Vec<u32>,
    pub modifier: u64,
    pub fourcc: u32,
    pub width: u32,
    pub height: u32,
}

/// Constraints advertised by `ext_image_copy_capture_session_v1`.
#[derive(Debug, Clone, Default)]
pub struct DmabufOffer {
    pub device: Option<libc::dev_t>,
    pub formats: Vec<(u32, Vec<u64>)>,
}

impl DmabufOffer {
    pub fn is_usable(&self) -> bool {
        self.device.is_some() && !self.formats.is_empty()
    }
}

pub struct DmabufBuffer {
    pub buffer: WlBuffer,
    _bo: BufferObject<()>,
    _device: GbmDevice<std::fs::File>,
    mmap: Option<(*mut u8, usize)>,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub fourcc: u32,
    pub modifier: u64,
}

// SAFETY: capture thread only.
unsafe impl Send for DmabufBuffer {}

impl DmabufBuffer {
    pub fn allocate<D>(
        dmabuf_global: &ZwpLinuxDmabufV1,
        qh: &QueueHandle<D>,
        offer: &DmabufOffer,
        width: u32,
        height: u32,
    ) -> Result<Self, String>
    where
        D: Dispatch<ZwpLinuxBufferParamsV1, ()> + Dispatch<WlBuffer, ()> + 'static,
    {
        if width == 0 || height == 0 {
            return Err("zero-sized dmabuf capture buffer".into());
        }
        let device_id = offer.device.ok_or("missing dmabuf device")?;
        let path = drm_path_for_dev(device_id)
            .ok_or_else(|| format!("no /dev/dri node for device {device_id}"))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|err| format!("open {}: {err}", path.display()))?;
        let device = GbmDevice::new(file).map_err(|err| format!("gbm device: {err}"))?;

        let (fourcc, modifiers) =
            pick_format(&offer.formats).ok_or("no usable dmabuf format from compositor")?;
        let gbm_format = fourcc_to_gbm(fourcc).ok_or("unsupported dmabuf fourcc")?;
        let drm_mods: Vec<DrmModifier> = modifiers.iter().copied().map(DrmModifier::from).collect();

        let bo = device
            .create_buffer_object_with_modifiers2::<()>(
                width,
                height,
                gbm_format,
                drm_mods.iter().copied(),
                BufferObjectFlags::RENDERING | BufferObjectFlags::LINEAR,
            )
            .or_else(|_| {
                device.create_buffer_object_with_modifiers::<()>(
                    width,
                    height,
                    gbm_format,
                    drm_mods.iter().copied(),
                )
            })
            .map_err(|err| format!("gbm create_buffer: {err}"))?;

        let modifier = u64::from(bo.modifier());
        let plane_count = bo.plane_count().max(1) as i32;
        let params = dmabuf_global.create_params(qh, ());
        for plane in 0..plane_count {
            let fd = bo
                .fd_for_plane(plane)
                .map_err(|err| format!("dmabuf plane fd: {err}"))?;
            let offset = bo.offset(plane);
            let stride = bo.stride_for_plane(plane);
            params.add(
                fd.as_fd(),
                plane as u32,
                offset,
                stride,
                (modifier >> 32) as u32,
                (modifier & 0xffff_ffff) as u32,
            );
        }

        let buffer = params.create_immed(
            width as i32,
            height as i32,
            fourcc,
            zwp_linux_buffer_params_v1::Flags::empty(),
            qh,
            (),
        );
        params.destroy();

        let stride = bo.stride_for_plane(0);
        let mmap = try_mmap_bo(&bo, stride, height);

        Ok(Self {
            buffer,
            _bo: bo,
            _device: device,
            mmap,
            width,
            height,
            stride,
            fourcc,
            modifier,
        })
    }

    pub fn pixels(&self) -> Option<&[u8]> {
        let (ptr, len) = self.mmap?;
        Some(unsafe { std::slice::from_raw_parts(ptr, len) })
    }

    /// Duplicate plane fds for a PipeWire push (caller owns the returned fds).
    pub fn export_planes(&self) -> Result<DmabufPlanes, String> {
        let plane_count = self._bo.plane_count().max(1) as i32;
        let mut fds = Vec::new();
        let mut offsets = Vec::new();
        let mut strides = Vec::new();
        for plane in 0..plane_count {
            let fd = self
                ._bo
                .fd_for_plane(plane)
                .map_err(|err| format!("export plane fd: {err}"))?;
            fds.push(fd);
            offsets.push(self._bo.offset(plane));
            strides.push(self._bo.stride_for_plane(plane));
        }
        Ok(DmabufPlanes {
            fds,
            offsets,
            strides,
            modifier: self.modifier,
            fourcc: self.fourcc,
            width: self.width,
            height: self.height,
        })
    }
}

impl Drop for DmabufBuffer {
    fn drop(&mut self) {
        if let Some((ptr, len)) = self.mmap.take() {
            unsafe {
                libc::munmap(ptr as *mut _, len);
            }
        }
        self.buffer.destroy();
    }
}

pub fn parse_dev_t(bytes: &[u8]) -> Option<libc::dev_t> {
    match bytes.len() {
        8 => {
            let v = u64::from_ne_bytes(bytes.try_into().ok()?);
            Some(v as libc::dev_t)
        }
        4 => {
            let v = u32::from_ne_bytes(bytes.try_into().ok()?);
            Some(v as libc::dev_t)
        }
        _ => None,
    }
}

fn drm_path_for_dev(dev: libc::dev_t) -> Option<PathBuf> {
    let dir = std::fs::read_dir("/dev/dri").ok()?;
    for entry in dir.flatten() {
        let path = entry.path();
        let meta = std::fs::metadata(&path).ok()?;
        if meta.rdev() == dev {
            return Some(path);
        }
    }
    None
}

fn pick_format(formats: &[(u32, Vec<u64>)]) -> Option<(u32, Vec<u64>)> {
    let preferred = [
        DrmFourcc::Xrgb8888 as u32,
        DrmFourcc::Argb8888 as u32,
        DrmFourcc::Xbgr8888 as u32,
        DrmFourcc::Abgr8888 as u32,
    ];
    for code in preferred {
        if let Some((_, mods)) = formats.iter().find(|(c, _)| *c == code) {
            let mut mods = mods.clone();
            // Prefer linear so we can mmap for MemFd fallback.
            let linear = u64::from(DrmModifier::Linear);
            if mods.contains(&linear) {
                mods.retain(|m| *m == linear);
            }
            return Some((code, mods));
        }
    }
    formats.first().map(|(c, m)| (*c, m.clone()))
}

fn fourcc_to_gbm(fourcc: u32) -> Option<GbmFormat> {
    match DrmFourcc::try_from(fourcc).ok()? {
        DrmFourcc::Xrgb8888 => Some(GbmFormat::Xrgb8888),
        DrmFourcc::Argb8888 => Some(GbmFormat::Argb8888),
        DrmFourcc::Xbgr8888 => Some(GbmFormat::Xbgr8888),
        DrmFourcc::Abgr8888 => Some(GbmFormat::Abgr8888),
        _ => None,
    }
}

fn try_mmap_bo(bo: &BufferObject<()>, stride: u32, height: u32) -> Option<(*mut u8, usize)> {
    let size = (stride as usize).checked_mul(height as usize)?;
    let fd = bo.fd_for_plane(0).ok()?;
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        None
    } else {
        Some((ptr as *mut u8, size))
    }
}

/// No-op handler for linux-dmabuf params events (we use create_immed).
pub fn ignore_params_event(
    _params: &ZwpLinuxBufferParamsV1,
    event: zwp_linux_buffer_params_v1::Event,
) {
    if let zwp_linux_buffer_params_v1::Event::Failed = event {
        eprintln!("linux-dmabuf params.create_immed failed");
    }
}

/// Helper so callers can match on `WEnum` modifier arrays from the session.
pub fn modifiers_from_array(array: &[u8]) -> Vec<u64> {
    array
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| u64::from_ne_bytes(*c))
        .collect()
}

pub fn fourcc_name(fourcc: u32) -> String {
    DrmFourcc::try_from(fourcc)
        .map(|f| format!("{f:?}"))
        .unwrap_or_else(|_| format!("0x{fourcc:08x}"))
}

pub fn format_is_bgr_order(fourcc: u32) -> bool {
    matches!(
        DrmFourcc::try_from(fourcc).ok(),
        Some(DrmFourcc::Xrgb8888 | DrmFourcc::Argb8888)
    )
}
