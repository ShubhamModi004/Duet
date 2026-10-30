//! Screen capture into a WebRTC video source.
//!
//! Uses libwebrtc's own desktop capturer (ScreenCaptureKit on macOS,
//! DXGI/Windows.Graphics.Capture on Windows), converts BGRA frames to I420 and
//! hands them to a [`NativeVideoSource`] that every guest's track shares.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use libwebrtc::desktop_capturer::{
    DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions,
};
use libwebrtc::native::yuv_helper;
use libwebrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use libwebrtc::video_source::native::NativeVideoSource;
use libwebrtc::video_source::VideoResolution;

pub const DEFAULT_FPS: u32 = 30;

pub struct ScreenCapture {
    source: NativeVideoSource,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ScreenCapture {
    /// Starts capturing the primary display at `fps`.
    pub fn start(fps: u32) -> anyhow::Result<Self> {
        let source = NativeVideoSource::new(VideoResolution { width: 1920, height: 1080 }, true);
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();

        let thread = {
            let source = source.clone();
            let stop = stop.clone();
            std::thread::Builder::new().name("duet-capture".into()).spawn(move || {
                let mut options = DesktopCapturerOptions::new(DesktopCaptureSourceType::Screen);
                options.set_include_cursor(true);
                let Some(mut capturer) = DesktopCapturer::new(options) else {
                    let _ = ready_tx.send(Err(anyhow::anyhow!(
                        "screen capture unavailable (on macOS, allow Duet in System Settings → Privacy & Security → Screen Recording)"
                    )));
                    return;
                };
                let screen = capturer.get_source_list().into_iter().next();
                let started = Instant::now();
                capturer.start_capture(screen, move |result| {
                    let Ok(frame) = result else { return };
                    let (w, h) = (frame.width(), frame.height());
                    if w <= 0 || h <= 0 {
                        return;
                    }
                    // I420 needs even dimensions.
                    let (ew, eh) = (w & !1, h & !1);
                    let mut buf = I420Buffer::new(ew as u32, eh as u32);
                    let (sy, su, sv) = buf.strides();
                    let (y, u, v) = buf.data_mut();
                    // DesktopFrame is BGRA in memory, which libyuv calls ARGB.
                    yuv_helper::argb_to_i420(
                        frame.data(),
                        frame.stride(),
                        y,
                        sy,
                        u,
                        su,
                        v,
                        sv,
                        ew,
                        eh,
                    );
                    let mut vf = VideoFrame::new(VideoRotation::VideoRotation0, buf);
                    vf.timestamp_us = started.elapsed().as_micros() as i64;
                    source.capture_frame(&vf);
                });
                let _ = ready_tx.send(Ok(()));

                let interval = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
                while !stop.load(Ordering::Relaxed) {
                    let t = Instant::now();
                    capturer.capture_frame();
                    if let Some(rest) = interval.checked_sub(t.elapsed()) {
                        std::thread::sleep(rest);
                    }
                }
            })?
        };

        ready_rx.recv().unwrap_or_else(|_| Err(anyhow::anyhow!("capture thread exited")))?;
        Ok(Self { source, stop, thread: Some(thread) })
    }

    pub fn source(&self) -> NativeVideoSource {
        self.source.clone()
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
