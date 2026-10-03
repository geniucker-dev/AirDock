use anyhow::{Result, ensure};
use ffmpeg::frame::Video;
use ffmpeg_next as ffmpeg;
use sdl2::{
    pixels::PixelFormatEnum,
    rect::Rect,
    render::{Canvas, Texture, TextureCreator},
    video::{Window, WindowContext},
};
use std::ptr;

pub fn colorspace(frame: &Video) -> i32 {
    use ffmpeg::ffi::AVColorSpace::*;
    match unsafe { (*frame.as_ptr()).colorspace } {
        AVCOL_SPC_FCC => ffmpeg::ffi::SWS_CS_FCC,
        AVCOL_SPC_BT470BG | AVCOL_SPC_SMPTE170M => ffmpeg::ffi::SWS_CS_ITU601,
        AVCOL_SPC_SMPTE240M => ffmpeg::ffi::SWS_CS_SMPTE240M,
        AVCOL_SPC_BT2020_NCL | AVCOL_SPC_BT2020_CL => ffmpeg::ffi::SWS_CS_BT2020,
        _ => ffmpeg::ffi::SWS_CS_ITU709,
    }
}
pub struct Converter {
    context: *mut ffmpeg::ffi::SwsContext,
    output: Option<Video>,
}
impl Default for Converter {
    fn default() -> Self {
        Self {
            context: ptr::null_mut(),
            output: None,
        }
    }
}
impl Converter {
    pub fn pitch(&self) -> usize {
        self.output.as_ref().map(|f| f.stride(0)).unwrap_or(0)
    }
    pub fn bgra<'a>(&'a mut self, frame: &Video) -> Result<&'a [u8]> {
        let (w, h) = (frame.width(), frame.height());
        ensure!(
            w > 0 && h > 0 && w <= 8192 && h <= 8192,
            "Invalid frame dimensions"
        );
        unsafe {
            self.context = ffmpeg::ffi::sws_getCachedContext(
                self.context,
                w as i32,
                h as i32,
                frame.format().into(),
                w as i32,
                h as i32,
                ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_BGRA,
                ffmpeg::ffi::SWS_FAST_BILINEAR,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            );
            ensure!(!self.context.is_null(), "Color converter unavailable");
            let coeffs = ffmpeg::ffi::sws_getCoefficients(colorspace(frame));
            let range = i32::from(
                (*frame.as_ptr()).color_range != ffmpeg::ffi::AVColorRange::AVCOL_RANGE_MPEG,
            );
            ensure!(
                ffmpeg::ffi::sws_setColorspaceDetails(
                    self.context,
                    coeffs,
                    range,
                    coeffs,
                    1,
                    0,
                    1 << 16,
                    1 << 16
                ) >= 0,
                "Color configuration failed"
            );
            if self
                .output
                .as_ref()
                .is_none_or(|f| f.width() != w || f.height() != h)
            {
                self.output = Some(Video::new(ffmpeg::format::Pixel::BGRA, w, h));
            }
            // FFmpeg allocations supply alignment and SIMD padding, including
            // for narrow/odd frames. A width*height Vec does not provide that.
            let output = self.output.as_mut().unwrap().as_mut_ptr();
            let f = frame.as_ptr();
            let lines = ffmpeg::ffi::sws_scale(
                self.context,
                (*f).data.as_ptr().cast(),
                (*f).linesize.as_ptr(),
                0,
                h as i32,
                (*output).data.as_mut_ptr(),
                (*output).linesize.as_ptr(),
            );
            ensure!(lines == h as i32, "Incomplete color conversion");
        }
        Ok(self.output.as_ref().unwrap().data(0))
    }
}
impl Drop for Converter {
    fn drop(&mut self) {
        unsafe { ffmpeg::ffi::sws_freeContext(self.context) }
    }
}

pub struct Display<'a> {
    texture: Option<Texture<'a>>,
    dimensions: (u32, u32),
    format: PixelFormatEnum,
    converter: Converter,
}
impl Default for Display<'_> {
    fn default() -> Self {
        Self {
            texture: None,
            dimensions: (0, 0),
            format: PixelFormatEnum::BGRA32,
            converter: Converter::default(),
        }
    }
}
impl<'a> Display<'a> {
    pub fn upload(
        &mut self,
        creator: &'a TextureCreator<WindowContext>,
        frame: &Video,
    ) -> Result<()> {
        // Keep the baseline's explicit swscale path for every mirror matrix/range.
        // Texture upload uses the single reusable buffer; AVFrame stays refcounted.
        let format = PixelFormatEnum::BGRA32;
        if self.dimensions != (frame.width(), frame.height())
            || self.format != format
            || self.texture.is_none()
        {
            self.texture =
                Some(creator.create_texture_streaming(format, frame.width(), frame.height())?);
            self.dimensions = (frame.width(), frame.height());
            self.format = format;
        }
        self.converter.bgra(frame)?;
        let pitch = self.converter.pitch();
        let bytes = self.converter.output.as_ref().unwrap().data(0);
        self.texture.as_mut().unwrap().update(None, bytes, pitch)?;
        Ok(())
    }
    pub fn draw(&self, canvas: &mut Canvas<Window>, viewport: Rect) -> Result<()> {
        if let Some(t) = &self.texture {
            let r = fit(self.dimensions, viewport);
            canvas.copy(t, None, r).map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }
    pub fn clear(&mut self) {
        self.texture = None;
        self.dimensions = (0, 0);
    }
}
pub fn fit(dimensions: (u32, u32), viewport: Rect) -> Rect {
    let ratio = (viewport.width() as f64 / dimensions.0 as f64)
        .min(viewport.height() as f64 / dimensions.1 as f64);
    let w = (dimensions.0 as f64 * ratio).round() as u32;
    let h = (dimensions.1 as f64 * ratio).round() as u32;
    Rect::new(
        viewport.x() + (viewport.width() as i32 - w as i32) / 2,
        viewport.y() + (viewport.height() as i32 - h as i32) / 2,
        w,
        h,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffmpeg::format::Pixel;
    #[test]
    fn portrait_keeps_aspect_ratio() {
        let r = fit((1080, 1920), Rect::new(200, 40, 900, 600));
        assert_eq!((r.width(), r.height()), (338, 600));
    }
    #[test]
    fn explicit_full_709_differs_from_full_601() {
        let mut f = Video::new(Pixel::YUV420P, 4, 4);
        f.data_mut(0).fill(140);
        f.data_mut(1).fill(90);
        f.data_mut(2).fill(210);
        let mut c = Converter::default();
        unsafe {
            (*f.as_mut_ptr()).colorspace = ffmpeg::ffi::AVColorSpace::AVCOL_SPC_BT709;
            (*f.as_mut_ptr()).color_range = ffmpeg::ffi::AVColorRange::AVCOL_RANGE_JPEG;
        }
        let bt709 = c.bgra(&f).unwrap()[..4].to_vec();
        unsafe {
            (*f.as_mut_ptr()).colorspace = ffmpeg::ffi::AVColorSpace::AVCOL_SPC_SMPTE170M;
        }
        let bt601 = c.bgra(&f).unwrap()[..4].to_vec();
        assert_ne!(bt709, bt601);
    }
    #[test]
    fn yuv_reference_matrices_ranges_and_formats_4752_cases() {
        use ffmpeg::ffi::{AVColorRange::*, AVColorSpace::*};
        let mut cases = 0;
        let mut maximum = 0f64;
        let mut rng = 1234u32;
        for format in [Pixel::YUV420P, Pixel::NV12] {
            for matrix in [AVCOL_SPC_BT709, AVCOL_SPC_SMPTE170M, AVCOL_SPC_UNSPECIFIED] {
                for range in [AVCOL_RANGE_MPEG, AVCOL_RANGE_JPEG, AVCOL_RANGE_UNSPECIFIED] {
                    let mut f = Video::new(format, 16, 16);
                    unsafe {
                        (*f.as_mut_ptr()).colorspace = matrix;
                        (*f.as_mut_ptr()).color_range = range;
                    }
                    let mut samples = vec![
                        [0, 128, 128],
                        [255, 128, 128],
                        [16, 128, 128],
                        [235, 128, 128],
                        [100, 90, 200],
                        [81, 90, 240],
                        [145, 54, 34],
                        [41, 240, 110],
                    ];
                    for _ in 0..256 {
                        let mut v = [0; 3];
                        for item in &mut v {
                            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
                            *item = (rng >> 24) as u8;
                        }
                        samples.push(v);
                    }
                    let mut c = Converter::default();
                    for [y, u, v] in samples {
                        f.data_mut(0).fill(y);
                        if format == Pixel::NV12 {
                            for pair in f.data_mut(1).as_chunks_mut::<2>().0 {
                                pair.copy_from_slice(&[u, v]);
                            }
                        } else {
                            f.data_mut(1).fill(u);
                            f.data_mut(2).fill(v);
                        }
                        let rgb = c.bgra(&f).unwrap();
                        let full = range != AVCOL_RANGE_MPEG;
                        let bt601 = matrix == AVCOL_SPC_SMPTE170M;
                        let yy = if full {
                            y as f64
                        } else {
                            (y as f64 - 16.) * 255. / 219.
                        };
                        let uu = (u as f64 - 128.) * if full { 1. } else { 255. / 224. };
                        let vv = (v as f64 - 128.) * if full { 1. } else { 255. / 224. };
                        let expected = [
                            yy + if bt601 { 1.772 } else { 1.8556 } * uu,
                            yy - if bt601 { 0.344136 } else { 0.187324 } * uu
                                - if bt601 { 0.714136 } else { 0.468124 } * vv,
                            yy + if bt601 { 1.402 } else { 1.5748 } * vv,
                        ];
                        for channel in 0..3 {
                            let error =
                                (rgb[channel] as f64 - expected[channel].clamp(0., 255.)).abs();
                            maximum = maximum.max(error);
                            assert!(error < 3., "{format:?}/{matrix:?}/{range:?}: {error}");
                        }
                        assert_eq!(rgb[3], 255);
                        cases += 1;
                    }
                }
            }
        }
        assert_eq!(cases, 4752);
        eprintln!("4752 independent YUV cases: max error {maximum:.3}/255");
    }
}
