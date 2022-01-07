use std::{path::Path, sync::Once, time::Duration};

use gstreamer::{
    prelude::{Cast, ElementExtManual, ObjectExt},
    traits::ElementExt,
    ElementFactory, MessageView,
};
use image::RgbImage;

static GST_INIT: Once = Once::new();

/// This toggles a library-internal flag that gstreamer has already been initiated.
pub fn assume_gst_init() {
    GST_INIT.call_once(|| {})
}

fn check_or_init_gst() {
    GST_INIT.call_once(|| gstreamer::init().expect("failed to initialize gst"))
}

struct VideoSequenceInner {
    pipeline: gstreamer::Element,
    appsink: gstreamer_app::AppSink,
}

impl VideoSequenceInner {
    fn set_state_with_timeout(
        &self,
        state: gstreamer::State,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        match self.pipeline.set_state(state)? {
            gstreamer::StateChangeSuccess::Success => Ok(()),
            gstreamer::StateChangeSuccess::Async => self.wait_async_done(timeout),
            gstreamer::StateChangeSuccess::NoPreroll => {
                Err(anyhow::anyhow!("live sources not supported"))
            }
        }
    }

    fn wait_async_done(&self, timeout: Duration) -> anyhow::Result<()> {
        loop {
            let msg = self
                .pipeline
                .bus()
                .expect("bus exists on pipeline")
                .timed_pop(Some(timeout.try_into()?));

            if let Some(msg) = msg {
                match msg.view() {
                    MessageView::AsyncDone(_) => return Ok(()),
                    MessageView::Error(err) => return Err(err.error().into()),
                    _ => {}
                }
            } else {
                return Err(anyhow::anyhow!("Timed out waiting for ASYNC_DONE"));
            }
        }
    }
}

impl Drop for VideoSequenceInner {
    fn drop(&mut self) {
        self.pipeline.set_state(gstreamer::State::Null).unwrap();
    }
}

/// The primary struct, encapsulates an opened video.
///
/// Keep in mind that, at least in this version, video-seeking is not exactly perfect;
/// - it assumes a constant frame rate over the video, any divergence or "lag" can mess up the total assumed frames
/// - it does this based on converted frame duration, together with above assumption, this may lead to skipped or duplicate frames
/// - the assumed total amount of frames may "overshoot", and frames at the end of the video may not be "there"
pub struct VideoSequence {
    inner: VideoSequenceInner,

    per_frame: Duration,
    frames: u64,
    current_index: u64,
}

impl VideoSequence {
    /// Open a video file and initialize gstreamer objects.
    ///
    /// A bunch of things can go wrong;
    /// - the wrong file was supplied
    /// - the file was not a video file
    /// - the right gstreamer plugins are not installed to
    /// - gstreamer borks itself
    pub fn open<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let uri = format!(
            "file://{}",
            path.as_ref()
                .canonicalize()?
                .to_str()
                .ok_or(anyhow::anyhow!("path cannot be a string"))?
        );

        check_or_init_gst();

        let pipeline = ElementFactory::make("playbin", None)?;

        pipeline.set_property("uri", uri)?;
        pipeline.set_property(
            "audio-sink",
            ElementFactory::make("fakesink", Some("fakeaudio"))?,
        )?;

        let videocaps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "RGB")
            .build();

        let appsink = ElementFactory::make("appsink", None)
            .map_err(|_| anyhow::anyhow!("appsink is missing"))?
            .dynamic_cast::<gstreamer_app::AppSink>()
            .expect("Sink element is expected to be an appsink!");

        appsink.set_property("caps", videocaps)?;
        pipeline.set_property("video-sink", appsink.clone())?;

        let inner = VideoSequenceInner { pipeline, appsink };

        inner.set_state_with_timeout(gstreamer::State::Paused, Duration::from_secs(10))?;

        let sample = inner.appsink.pull_preroll()?;

        let caps = sample
            .caps_owned()
            .ok_or(anyhow::anyhow!("No data in video"))?;

        let struc = caps.structure(0).expect("caps has structure");

        let fraction: gstreamer::Fraction = struc
            .get("framerate")
            .map_err(|_| anyhow::anyhow!("Could not determine frame rate for seeking"))?;

        let num = *fraction.0.numer();

        let denom = *fraction.0.denom();

        let g_sec: Duration = gstreamer::ClockTime::SECOND.into();

        let per_frame: Duration = g_sec.mul_f32(denom as f32).div_f32(num as f32);

        let duration: gstreamer::ClockTime = inner
            .pipeline
            .query_duration()
            .ok_or(anyhow::anyhow!("Could not determine duration of video"))?;

        let duration: Duration = duration.into();

        let frames = (duration.as_nanos() / per_frame.as_nanos()) as u64;

        let mut s = Self {
            inner,
            per_frame,
            frames,
            current_index: 0,
        };

        s.raw_seek(0)?;

        return Ok(s);
    }

    fn raw_seek(&mut self, index: u64) -> anyhow::Result<()> {
        use gstreamer::{ClockTime, SeekFlags, SeekType};

        if index > self.frames {
            return Err(anyhow::anyhow!("frame range exceeds file duration"));
        }

        let timestamp: ClockTime = self.per_frame.mul_f64(index as f64).try_into()?;

        let flags = SeekFlags::ACCURATE | SeekFlags::FLUSH;

        self.inner
            .pipeline
            .seek(
                1.0,
                flags,
                SeekType::Set,
                timestamp,
                SeekType::None,
                ClockTime::ZERO,
            )
            .map_err(|e| anyhow::anyhow!("seek event not handled: {}", e))?;

        self.inner.wait_async_done(Duration::from_secs(10))?;

        self.current_index = index;

        Ok(())
    }

    fn step(&mut self, count: u64) -> anyhow::Result<()> {
        if count == 0 {
            return Ok(());
        }

        use gstreamer::ClockTime;

        let step_dur: ClockTime = self.per_frame.mul_f64(count as f64).try_into()?;

        let ev = gstreamer::event::Step::new(step_dur, 1.0, true, false);

        if !self.inner.pipeline.send_event(ev) {
            return Err(anyhow::anyhow!("Step event not handled"));
        }

        self.inner.wait_async_done(Duration::from_secs(10))?;

        self.current_index = self.current_index + count;

        Ok(())
    }

    fn seek(&mut self, index: u64) -> anyhow::Result<()> {
        if index < self.current_index {
            self.raw_seek(index)
        } else if index > self.current_index {
            let delta = index - self.current_index;

            const MAX_DELTA: u64 = 1;

            if delta > MAX_DELTA {
                self.raw_seek(index)
            } else {
                self.step(delta)
            }
        } else {
            Ok(())
        }
    }

    /// Does its best to grab the frame at a frame index, see struct documentation for caveats.
    ///
    /// Can return a "Failed to pull preroll sample" error to note that frame at current index is not available.
    pub fn get_frame(&mut self, index: u64) -> anyhow::Result<Option<RgbImage>> {
        self.seek(index)?;

        let sample = self.inner.appsink.pull_preroll()?;

        if sample.buffer().is_none() {
            return Ok(None);
        }

        convert_sample_to_image(sample).map(|i| Some(i))
    }

    /// Assumed amount of frames in this sequence, see struct documentation for caveats.
    pub fn len(&self) -> u64 {
        self.frames
    }
}

/// Converts a single RGB frame sample to an `image::RgbImage`
pub fn convert_sample_to_image(sample: gstreamer::Sample) -> anyhow::Result<RgbImage> {
    let caps = sample
        .caps()
        .ok_or(anyhow::anyhow!("could not grab caps"))?;
    let buffer = sample
        .buffer()
        .ok_or(anyhow::anyhow!("could not grab buffer"))?;

    let mut buf = vec![0u8; buffer.size()];

    buffer
        .copy_to_slice(0, &mut buf)
        .map_err(|_| anyhow::anyhow!("could not copy full image buffer"))?;

    let struc = caps.structure(0).expect("caps has structure");

    let width: i32 = struc.get("width")?;
    let height: i32 = struc.get("height")?;
    let format: String = struc.get("format")?;

    if format != "RGB" {
        return Err(anyhow::anyhow!("Need RGB frame sample to convert to image"));
    }

    RgbImage::from_raw(width as u32, height as u32, buf)
        .ok_or(anyhow::anyhow!("image buffer was not sufficient"))
}
