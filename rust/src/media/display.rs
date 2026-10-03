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
    chroma: Option<Video>,
}
impl Default for Converter {
    fn default() -> Self {
        Self {
            context: ptr::null_mut(),
            output: None,
            chroma: None,
        }
    }
}
impl Converter {
    pub fn pitch(&self) -> usize {
        self.output.as_ref().map(|f| f.stride(0)).unwrap_or(0)
    }
    pub fn bgra<'a>(&'a mut self, frame: &Video) -> Result<&'a [u8]> {
        self.bgra_with_planar_nv12(frame, cfg!(windows))
    }
    fn bgra_with_planar_nv12<'a>(
        &'a mut self,
        frame: &Video,
        planar_nv12: bool,
    ) -> Result<&'a [u8]> {
        let (w, h) = (frame.width(), frame.height());
        ensure!(
            w > 0 && h > 0 && w <= 8192 && h <= 8192,
            "Invalid frame dimensions"
        );
        let (mut data, mut strides) = unsafe {
            let f = frame.as_ptr();
            ((*f).data, (*f).linesize)
        };
        let mut format = frame.format();
        if planar_nv12 && format == ffmpeg::format::Pixel::NV12 {
            // MSVC cannot compile swscale's inline-assembly NV12 RGB path.
            // Its C fallback loses precision even for limited-range white.
            // Split only UV into cached planes, retaining the original luma
            // pointer, so the existing accelerated planar RGB path can run.
            let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
            if self
                .chroma
                .as_ref()
                .is_none_or(|c| c.width() != cw || c.height() != ch * 2)
            {
                self.chroma = Some(Video::new(ffmpeg::format::Pixel::GRAY8, cw, ch * 2));
            }
            let chroma = self.chroma.as_mut().unwrap();
            let pitch = chroma.stride(0);
            let (u, v) = chroma.data_mut(0).split_at_mut(pitch * ch as usize);
            let source = frame.data(1);
            let source_pitch = frame.stride(1);
            for row in 0..ch as usize {
                split_uv(
                    &source[row * source_pitch..row * source_pitch + cw as usize * 2],
                    &mut u[row * pitch..row * pitch + cw as usize],
                    &mut v[row * pitch..row * pitch + cw as usize],
                );
            }
            data[1] = u.as_mut_ptr();
            data[2] = v.as_mut_ptr();
            strides[1] = pitch as i32;
            strides[2] = pitch as i32;
            format = ffmpeg::format::Pixel::YUV420P;
        } else {
            self.chroma = None;
        }
        unsafe {
            self.context = ffmpeg::ffi::sws_getCachedContext(
                self.context,
                w as i32,
                h as i32,
                format.into(),
                w as i32,
                h as i32,
                ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_BGRA,
                ffmpeg::software::scaling::Flags::FAST_BILINEAR.bits(),
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
            let lines = ffmpeg::ffi::sws_scale(
                self.context,
                data.as_ptr().cast(),
                strides.as_ptr(),
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

fn split_uv(source: &[u8], u: &mut [u8], v: &mut [u8]) {
    assert_eq!(source.len(), u.len() * 2);
    assert_eq!(u.len(), v.len());
    let processed = {
        #[cfg(target_arch = "x86_64")]
        {
            use std::arch::x86_64::*;
            let mut i = 0;
            // SSE2 is mandatory on x86-64. Each iteration reads exactly 32
            // interleaved bytes and writes 16 bytes to each separate plane.
            unsafe {
                let mask = _mm_set1_epi16(255);
                while i + 16 <= u.len() {
                    let a = _mm_loadu_si128(source.as_ptr().add(i * 2).cast());
                    let b = _mm_loadu_si128(source.as_ptr().add(i * 2 + 16).cast());
                    let uu = _mm_packus_epi16(_mm_and_si128(a, mask), _mm_and_si128(b, mask));
                    let vv = _mm_packus_epi16(_mm_srli_epi16(a, 8), _mm_srli_epi16(b, 8));
                    _mm_storeu_si128(u.as_mut_ptr().add(i).cast(), uu);
                    _mm_storeu_si128(v.as_mut_ptr().add(i).cast(), vv);
                    i += 16;
                }
            }
            i
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            0
        }
    };
    for i in processed..u.len() {
        u[i] = source[i * 2];
        v[i] = source[i * 2 + 1];
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
        check_yuv_reference(cfg!(windows));
    }
    #[test]
    fn planar_nv12_reference_matrices_ranges_and_formats_4752_cases() {
        check_yuv_reference(true);
    }
    fn check_yuv_reference(planar_nv12: bool) {
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
                        let rgb = c.bgra_with_planar_nv12(&f, planar_nv12).unwrap();
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
                            assert!(
                                error < 3.,
                                "{format:?}/{matrix:?}/{range:?} YUV={y},{u},{v} channel={channel}: {error}"
                            );
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
    #[test]
    fn split_uv_matches_scalar_for_unaligned_vectors_and_tails() {
        for count in 0..100 {
            let source: Vec<u8> = (0..count * 2 + 2).map(|i| (i * 37 + 13) as u8).collect();
            let mut u = vec![77; count + 2];
            let mut v = vec![88; count + 2];
            split_uv(
                &source[1..count * 2 + 1],
                &mut u[1..count + 1],
                &mut v[1..count + 1],
            );
            for i in 0..count {
                assert_eq!((u[i + 1], v[i + 1]), (source[i * 2 + 1], source[i * 2 + 2]));
            }
            assert_eq!((u[0], u[count + 1], v[0], v[count + 1]), (77, 77, 88, 88));
        }
    }
    #[test]
    fn nv12_staging_preserves_rows_odd_dimensions_and_reconfiguration() {
        let mut converter = Converter::default();
        for (w, h) in [(37, 17), (16, 16), (66, 10), (37, 17)] {
            let mut nv12 = Video::new(Pixel::NV12, w, h);
            let mut planar = Video::new(Pixel::YUV420P, w, h);
            for frame in [&mut nv12, &mut planar] {
                frame.set_color_space(ffmpeg::color::Space::BT709);
                frame.set_color_range(ffmpeg::color::Range::JPEG);
                let pitch = frame.stride(0);
                for row in 0..h as usize {
                    for column in 0..w as usize {
                        frame.data_mut(0)[row * pitch + column] = (row * 7 + column * 3) as u8;
                    }
                }
            }
            let source_pitch = nv12.stride(1);
            let u_pitch = planar.stride(1);
            let v_pitch = planar.stride(2);
            for row in 0..h.div_ceil(2) as usize {
                for column in 0..w.div_ceil(2) as usize {
                    let (u, v) = (
                        (row * 41 + column * 17) as u8,
                        (row * 11 + column * 53) as u8,
                    );
                    nv12.data_mut(1)
                        [row * source_pitch + column * 2..row * source_pitch + column * 2 + 2]
                        .copy_from_slice(&[u, v]);
                    planar.data_mut(1)[row * u_pitch + column] = u;
                    planar.data_mut(2)[row * v_pitch + column] = v;
                }
            }
            let original_y = nv12.data(0).to_vec();
            let actual = converter
                .bgra_with_planar_nv12(&nv12, true)
                .unwrap()
                .to_vec();
            let actual_pitch = converter.pitch();
            let mut reference = Converter::default();
            let expected = reference.bgra(&planar).unwrap().to_vec();
            for row in 0..h as usize {
                assert_eq!(
                    &actual[row * actual_pitch..row * actual_pitch + w as usize * 4],
                    &expected[row * reference.pitch()..row * reference.pitch() + w as usize * 4]
                );
            }
            assert_eq!(nv12.data(0), original_y);
            let chroma = converter.chroma.as_ref().unwrap();
            assert_eq!(
                (chroma.width(), chroma.height()),
                (w.div_ceil(2), h.div_ceil(2) * 2)
            );
        }
    }
}
