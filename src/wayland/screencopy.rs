//! ext-image-copy-capture: screenshots and screen recording by other apps (grim,
//! wf-recorder, xdg-desktop-portal-wlr for screen sharing in browsers).
//!
//! A client picks a *source* (an output), opens a *session* on it, gets told what kind of
//! buffer to bring (size and pixel formats), and then asks for *frames*: for each one we
//! copy the next composited frame — the full 3D view with overlays — into its buffer.
//!
//! Two protocols do this: the standard ext-image-copy-capture (grim, portals) and the
//! older wlr-screencopy (wf-recorder, OBS's wlrobs), implemented here by hand since
//! smithay doesn't ship it. Both ride on our screenshot readback: whenever a frame is
//! waiting, the backend reads the finished framebuffer back and `deliver` writes it into
//! the client's shm buffer. Only shm buffers are offered; every capture tool supports them.

use std::time::Duration;

use smithay::{
    output::{Output, WeakOutput},
    reexports::wayland_server::protocol::wl_shm,
    utils::{IsAlive, Physical, Rectangle, Size, Transform},
    wayland::{
        image_capture_source::{
            ImageCaptureSource, ImageCaptureSourceHandler, OutputCaptureSourceHandler,
            OutputCaptureSourceState,
        },
        image_copy_capture::{
            BufferConstraints, Frame, FrameRef, ImageCopyCaptureHandler, ImageCopyCaptureState,
            Session, SessionRef,
        },
        shm::with_buffer_contents_mut,
    },
};

use smithay::reexports::{
    wayland_protocols_wlr::screencopy::v1::server::{
        zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
        zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
    },
    wayland_server::{
        backend::GlobalId, protocol::wl_buffer::WlBuffer, Client, DataInit, DisplayHandle, New,
        Resource,
    },
};
use std::sync::Mutex;

use crate::{render::screenshot::CapturedImage, state::SpaceState};

/// Capture state: open sessions and frames waiting for the next render.
#[derive(Default)]
pub struct Screencopy {
    sessions: Vec<Session>,
    pending: Vec<(Pending, Output)>,
}

enum Pending {
    Ext(Frame),
    Wlr(WlrCopy),
}

impl Screencopy {
    /// Does a frame wait for `output`? Then the backend must read the next frame back.
    pub fn wants(&self, output: &Output) -> bool {
        self.pending.iter().any(|(_, o)| o == output)
    }
}

fn source_output(source: &ImageCaptureSource) -> Option<Output> {
    source.user_data().get::<WeakOutput>()?.upgrade()
}

fn constraints(output: &Output) -> Option<BufferConstraints> {
    let mode = output.current_mode()?;
    Some(BufferConstraints {
        size: Size::from((mode.size.w, mode.size.h)),
        // 32-bit BGRA in memory (little endian ARGB/XRGB): what every tool can use.
        shm: vec![wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888],
        dma: None,
    })
}

impl ImageCaptureSourceHandler for SpaceState {}

impl OutputCaptureSourceHandler for SpaceState {
    fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState {
        &mut self.output_capture_source_state
    }

    fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
        source.user_data().insert_if_missing(|| output.downgrade());
    }
}

impl ImageCopyCaptureHandler for SpaceState {
    fn image_copy_capture_state(&mut self) -> &mut ImageCopyCaptureState {
        &mut self.image_copy_capture_state
    }

    fn capture_constraints(&mut self, source: &ImageCaptureSource) -> Option<BufferConstraints> {
        // The screen can't be captured while it is locked: a screenshot tool must never
        // see what the lock hides (nor the lock screen, for that matter).
        if self.lock.is_locked() {
            return None;
        }
        constraints(&source_output(source)?)
    }

    fn new_session(&mut self, session: Session) {
        tracing::info!(client = ?session.client().map(|c| c.id()), "screen capture session started");
        self.screencopy.sessions.retain(|s| s.alive());
        self.screencopy.sessions.push(session);
    }

    fn frame(&mut self, session: &SessionRef, frame: Frame) {
        use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_frame_v1::FailureReason;
        let Some(output) = source_output(&session.source()) else {
            frame.fail(FailureReason::Stopped);
            return;
        };
        if self.lock.is_locked() {
            frame.fail(FailureReason::Stopped);
            return;
        }
        self.screencopy.pending.push((Pending::Ext(frame), output));
        // A frame is due now, even if nothing on screen changed.
        self.view.dirty = true;
        self.queue_redraw();
    }

    fn frame_aborted(&mut self, frame: FrameRef) {
        self.screencopy.pending.retain(|(p, _)| !matches!(p, Pending::Ext(f) if **f == frame));
    }
}

/// Write the frame just rendered for `output` into every waiting client buffer.
pub fn deliver(state: &mut Screencopy, output: &Output, image: Result<&CapturedImage, &str>) {
    use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_frame_v1::FailureReason;
    let (mine, rest): (Vec<_>, Vec<_>) =
        std::mem::take(&mut state.pending).into_iter().partition(|(_, o)| o == output);
    state.pending = rest;
    for (pending, _) in mine {
        match pending {
            Pending::Ext(frame) => {
                let result = image.map_err(str::to_string).and_then(|img| {
                    copy_region(
                        &frame.buffer(),
                        img,
                        Rectangle::from_size((img.width as i32, img.height as i32).into()),
                    )
                });
                match result {
                    Ok(()) => frame.success(Transform::Normal, None, monotonic_now()),
                    Err(e) => {
                        tracing::warn!("screen capture: {e}");
                        frame.fail(FailureReason::BufferConstraints);
                    }
                }
            }
            Pending::Wlr(copy) => {
                let result = image
                    .map_err(str::to_string)
                    .and_then(|img| copy_region(&copy.buffer, img, copy.region));
                copy.finish(result);
            }
        }
    }
}

/// `region` of the RGBA frame (rows top first) -> the client's 32-bit shm buffer
/// (B, G, R, A in memory).
fn copy_region(
    buffer: &WlBuffer,
    image: &CapturedImage,
    region: Rectangle<i32, Physical>,
) -> Result<(), String> {
    let inside = Rectangle::from_size((image.width as i32, image.height as i32).into());
    if !inside.contains_rect(region) {
        return Err("capture region outside the frame".into());
    }
    with_buffer_contents_mut(buffer, |ptr, len, data| {
        let (w, h) = (region.size.w, region.size.h);
        if data.width != w || data.height != h {
            return Err(format!("buffer is {}x{}, frame is {w}x{h}", data.width, data.height));
        }
        if !matches!(data.format, wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888) {
            return Err(format!("unsupported format {:?}", data.format));
        }
        let stride = data.stride as usize;
        let offset = data.offset as usize;
        let needed = offset + stride * (h as usize - 1) + w as usize * 4;
        if stride < w as usize * 4 || needed > len {
            return Err("buffer too small".into());
        }
        // SAFETY: smithay hands us the mapped pool (`len` bytes); every write below is
        // inside offset..needed, checked above.
        let pool = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
        let image_stride = image.width as usize * 4;
        for y in 0..h as usize {
            let row = (region.loc.y as usize + y) * image_stride + region.loc.x as usize * 4;
            let src = &image.rgba[row..row + w as usize * 4];
            let dst = &mut pool[offset + y * stride..offset + y * stride + w as usize * 4];
            for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
                *d = [s[2], s[1], s[0], 255];
            }
        }
        Ok(())
    })
    .map_err(|e| format!("{e:?}"))?
}

fn monotonic_now() -> Duration {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
}

// ---------------------------------------------------------------------------------------
// wlr-screencopy-unstable-v1
//
// capture_output(frame, overlay_cursor, output) creates a frame object; we answer with
// the buffer it needs (format, size, stride; v3 also buffer_done). The client then calls
// copy(buffer) — "the next frame" — or copy_with_damage(buffer) — "the next frame that
// changes something", which is how recorders avoid encoding identical frames. We reply
// with flags + damage + ready, or failed.

const WLR_VERSION: u32 = 3;

/// The manager global.
pub struct WlrScreencopyGlobal;

pub fn create_wlr_global(dh: &DisplayHandle) -> GlobalId {
    dh.create_global::<SpaceState, ZwlrScreencopyManagerV1, _>(WLR_VERSION, WlrScreencopyGlobal)
}

impl smithay::wayland::GlobalDispatch2<ZwlrScreencopyManagerV1, SpaceState> for WlrScreencopyGlobal {
    fn bind(
        &self,
        _state: &mut SpaceState,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        data_init: &mut DataInit<'_, SpaceState>,
    ) {
        data_init.init(resource, WlrManager);
    }
}

pub struct WlrManager;

/// Per-frame object state.
pub struct WlrFrame {
    output: Option<Output>,
    /// The part of the output to capture, in output pixels.
    region: Rectangle<i32, Physical>,
    /// copy may only be called once per frame object.
    used: Mutex<bool>,
}

impl smithay::wayland::Dispatch2<ZwlrScreencopyManagerV1, SpaceState> for WlrManager {
    fn request(
        &self,
        state: &mut SpaceState,
        _client: &Client,
        _resource: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, SpaceState>,
    ) {
        let (frame, output, region) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput { frame, output, .. } => {
                (frame, output, None)
            }
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                output,
                x,
                y,
                width,
                height,
                ..
            } => (
                frame,
                output,
                Some(Rectangle::<i32, smithay::utils::Logical>::new(
                    (x, y).into(),
                    (width, height).into(),
                )),
            ),
            zwlr_screencopy_manager_v1::Request::Destroy => return,
            _ => return,
        };
        let output = Output::from_resource(&output);
        let full =
            output.as_ref().and_then(|o| o.current_mode()).map(|m| Rectangle::from_size(m.size));
        let region = match (full, &output, region) {
            (Some(full), _, None) => Some(full),
            (Some(full), Some(o), Some(logical)) => {
                // Regions come in logical coordinates of the output.
                let scale = o.current_scale().fractional_scale();
                logical.to_f64().to_physical(scale).to_i32_round().intersection(full)
            }
            _ => None,
        };
        let locked = state.lock.is_locked();
        let region = region.filter(|r| r.size.w > 0 && r.size.h > 0 && !locked);
        let frame = data_init.init(
            frame,
            WlrFrame {
                output: output.clone(),
                region: region.unwrap_or_default(),
                used: Mutex::new(false),
            },
        );
        let Some(region) = region else {
            frame.failed();
            return;
        };
        frame.buffer(
            wl_shm::Format::Xrgb8888,
            region.size.w as u32,
            region.size.h as u32,
            region.size.w as u32 * 4,
        );
        if frame.version() >= 3 {
            frame.buffer_done();
        }
    }
}

impl smithay::wayland::Dispatch2<ZwlrScreencopyFrameV1, SpaceState> for WlrFrame {
    fn request(
        &self,
        state: &mut SpaceState,
        _client: &Client,
        resource: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, SpaceState>,
    ) {
        let (buffer, with_damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            _ => return,
        };
        if let Ok(mut used) = self.used.lock() {
            if *used {
                resource
                    .post_error(zwlr_screencopy_frame_v1::Error::AlreadyUsed, "copy called twice");
                return;
            }
            *used = true;
        }
        let Some(output) = self.output.clone().filter(|_| !state.lock.is_locked()) else {
            resource.failed();
            return;
        };
        let size = self.region.size;
        let fits = smithay::wayland::shm::with_buffer_contents(&buffer, |_, _, d| {
            d.width == size.w
                && d.height == size.h
                && d.stride >= size.w * 4
                && matches!(d.format, wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888)
        });
        if !matches!(fits, Ok(true)) {
            resource.post_error(zwlr_screencopy_frame_v1::Error::InvalidBuffer, "wrong buffer");
            return;
        }
        let copy = WlrCopy { frame: resource.clone(), buffer, region: self.region, with_damage };
        state.screencopy.pending.push((Pending::Wlr(copy), output));
        if !with_damage {
            // copy: the next frame, now. copy_with_damage waits for something to change.
            state.view.dirty = true;
            state.queue_redraw();
        }
    }

    fn destroyed(
        &self,
        state: &mut SpaceState,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        resource: &ZwlrScreencopyFrameV1,
    ) {
        state
            .screencopy
            .pending
            .retain(|(p, _)| !matches!(p, Pending::Wlr(c) if &c.frame == resource));
    }
}

struct WlrCopy {
    frame: ZwlrScreencopyFrameV1,
    buffer: WlBuffer,
    region: Rectangle<i32, Physical>,
    with_damage: bool,
}

impl WlrCopy {
    fn finish(self, result: Result<(), String>) {
        if let Err(e) = result {
            tracing::warn!("wlr screencopy: {e}");
            self.frame.failed();
            return;
        }
        // Rows were written top first: no y-invert.
        self.frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
        if self.with_damage && self.frame.version() >= 2 {
            let s = self.region.size;
            self.frame.damage(0, 0, s.w as u32, s.h as u32);
        }
        let t = monotonic_now();
        let secs = t.as_secs();
        self.frame.ready((secs >> 32) as u32, secs as u32, t.subsec_nanos());
    }
}
