//! Screenshots of the composited frame.
//!
//! After a frame is rendered (and before it is presented), the backend reads the pixels
//! back from the framebuffer. PNG encoding happens on a worker thread so a big screenshot
//! never stalls the compositor; the result is reported back to whoever asked (IPC).

use std::{fs::OpenOptions, io::BufWriter, path::PathBuf};

use smithay::{
    backend::{allocator::Fourcc, renderer::ExportMem},
    reexports::calloop::channel::Sender,
    utils::{Physical, Rectangle, Size, Transform},
};

use crate::ipc::ConnectionId;

pub struct ScreenshotRequest {
    pub path: PathBuf,
    /// Overwrite an existing file.
    pub force: bool,
    /// IPC connection waiting for the result.
    pub reply_to: Option<ConnectionId>,
}

/// Outcome of a finished screenshot, sent back to the event loop.
pub struct ScreenshotDone {
    pub reply_to: Option<ConnectionId>,
    pub result: Result<(PathBuf, u32, u32), String>,
}

/// RGBA pixels, top row first.
pub struct CapturedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A readback that has been queued on the GPU but not yet read by the CPU.
pub struct PendingCapture<M> {
    mapping: M,
    size: Size<i32, Physical>,
    flip_rows: bool,
}

/// Queue a copy of the bound framebuffer into a GPU buffer (asynchronous).
///
/// Call this BEFORE presenting the frame (the back buffer is undefined afterwards), and
/// `finish_capture` AFTER presenting: mapping the copy may switch the EGL context to a
/// surfaceless one, which would make the following buffer swap fail with BAD_SURFACE.
///
/// Row order: glReadPixels returns rows bottom-up. smithay draws into textures top-down
/// (Transform::Normal) but into window surfaces with Transform::Flipped180 so they show
/// upright, so the output transform decides whether rows must be flipped.
pub fn start_capture<R: ExportMem>(
    renderer: &mut R,
    framebuffer: &R::Framebuffer<'_>,
    size: Size<i32, Physical>,
    transform: Transform,
) -> Result<PendingCapture<R::TextureMapping>, String> {
    let region = Rectangle::from_size((size.w, size.h).into());
    let mapping = renderer
        .copy_framebuffer(framebuffer, region, Fourcc::Abgr8888)
        .map_err(|e| format!("copy_framebuffer failed: {e:?}"))?;
    Ok(PendingCapture { mapping, size, flip_rows: transform == Transform::Flipped180 })
}

/// Read the queued copy into RGBA pixels, top row first.
pub fn finish_capture<R: ExportMem>(
    renderer: &mut R,
    pending: PendingCapture<R::TextureMapping>,
) -> Result<CapturedImage, String> {
    let data =
        renderer.map_texture(&pending.mapping).map_err(|e| format!("map_texture failed: {e:?}"))?;
    let (w, h) = (pending.size.w as usize, pending.size.h as usize);
    let stride = w * 4;
    if data.len() < stride * h {
        return Err(format!("short read: {} < {}", data.len(), stride * h));
    }
    let mut rgba = Vec::with_capacity(stride * h);
    for row in 0..h {
        let src = if pending.flip_rows { h - 1 - row } else { row };
        rgba.extend_from_slice(&data[src * stride..(src + 1) * stride]);
    }
    Ok(CapturedImage { width: w as u32, height: h as u32, rgba })
}

/// Encode and write on a worker thread, then report through `done`.
pub fn save_async(request: ScreenshotRequest, image: CapturedImage, done: Sender<ScreenshotDone>) {
    std::thread::spawn(move || {
        let result = write_png(&request.path, request.force, &image)
            .map(|()| (request.path.clone(), image.width, image.height));
        let _ = done.send(ScreenshotDone { reply_to: request.reply_to, result });
    });
}

fn write_png(path: &PathBuf, force: bool, image: &CapturedImage) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        // Atomic "fail if exists": no check-then-create race.
        options.create_new(true);
    }
    let file = options.open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    // The framebuffer has no meaningful alpha for an opaque desktop: force opaque so the
    // PNG looks like what is on screen.
    let mut rgba = image.rgba.clone();
    for px in rgba.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
    Ok(())
}
