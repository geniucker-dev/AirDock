// SPDX-License-Identifier: MPL-2.0
//! Validated 8/10-bit YUV plane descriptions. No CPU RGBA path.
use anyhow::{Result, bail, ensure};
use ffmpeg_next::{self as ffmpeg, format::Pixel, frame::Video};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Nv12,
    Yuv420p,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    Byte,
    P010,
    Planar10,
}
impl Storage {
    pub fn depth(self) -> u32 {
        if self == Self::Byte { 8 } else { 10 }
    }
    pub fn bytes(self) -> u32 {
        if self == Self::Byte { 1 } else { 2 }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub pointer: *const u8,
    pub stride: i32,
    pub width: u32,
    pub height: u32,
    pub bytes: u32,
}
#[derive(Clone, Copy, Debug)]
pub struct Planes {
    pub layout: Layout,
    pub storage: Storage,
    pub planes: [Plane; 3],
    pub width: u32,
    pub height: u32,
    pub matrix: [[f32; 4]; 3],
    pub color: super::color::Profile,
}
pub fn coefficients(frame: &Video) -> Result<[[f32; 4]; 3]> {
    use ffmpeg::ffi::{AVColorRange::*, AVColorSpace::*, AVColorTransferCharacteristic::*};
    let f = unsafe { &*frame.as_ptr() };
    let hdr = matches!(f.color_trc, AVCOL_TRC_SMPTE2084 | AVCOL_TRC_ARIB_STD_B67);
    let space = if hdr
        && f.colorspace == AVCOL_SPC_UNSPECIFIED
        && f.color_primaries != ffmpeg::ffi::AVColorPrimaries::AVCOL_PRI_BT709
    {
        AVCOL_SPC_BT2020_NCL
    } else {
        f.colorspace
    };
    let (kr, kb) = match space {
        AVCOL_SPC_FCC => (0.30, 0.11),
        AVCOL_SPC_BT470BG | AVCOL_SPC_SMPTE170M => (0.299, 0.114),
        AVCOL_SPC_SMPTE240M => (0.212, 0.087),
        AVCOL_SPC_BT2020_NCL => (0.2627, 0.0593),
        AVCOL_SPC_BT2020_CL => bail!("BT.2020 constant-luminance video is not supported"),
        AVCOL_SPC_BT709 | AVCOL_SPC_UNSPECIFIED => (0.2126, 0.0722),
        _ => bail!("Unsupported YUV colour matrix {:?}", f.colorspace),
    };
    let limited =
        f.color_range == AVCOL_RANGE_MPEG || (hdr && f.color_range == AVCOL_RANGE_UNSPECIFIED);
    let depth = if matches!(frame.format(), Pixel::P010LE | Pixel::YUV420P10LE) {
        10
    } else {
        8
    };
    let step = (1u32 << (depth - 8)) as f32;
    let maximum = ((1u32 << depth) - 1) as f32;
    let ys = if limited { maximum / (219. * step) } else { 1. };
    let cs = if limited { maximum / (224. * step) } else { 1. };
    let yo = if limited { 16. * step / maximum } else { 0. };
    let co = 128. * step / maximum;
    let kg = 1. - kr - kb;
    let ru = 0.;
    let rv = 2. * (1. - kr) * cs;
    let gu = -2. * kb * (1. - kb) / kg * cs;
    let gv = -2. * kr * (1. - kr) / kg * cs;
    let bu = 2. * (1. - kb) * cs;
    Ok([
        [ys, ru, rv, -ys * yo - co * (ru + rv)],
        [ys, gu, gv, -ys * yo - co * (gu + gv)],
        [ys, bu, 0., -ys * yo - co * bu],
    ])
}
pub fn describe(frame: &Video) -> Result<Planes> {
    let width = frame.width();
    let height = frame.height();
    ensure!(
        width > 0 && height > 0 && width <= 8192 && height <= 8192,
        "Invalid video dimensions"
    );
    let (layout, storage) = match frame.format() {
        Pixel::NV12 => (Layout::Nv12, Storage::Byte),
        Pixel::YUV420P | Pixel::YUVJ420P => (Layout::Yuv420p, Storage::Byte),
        Pixel::P010LE => (Layout::Nv12, Storage::P010),
        Pixel::YUV420P10LE => (Layout::Yuv420p, Storage::Planar10),
        other => bail!(
            "Unsupported video pixel format {other:?}; expected NV12/YUV420P, P010LE or YUV420P10LE"
        ),
    };
    let matrix = coefficients(frame)?;
    let f = unsafe { &*frame.as_ptr() };
    let mut planes = [Plane {
        pointer: std::ptr::null(),
        stride: 0,
        width: 0,
        height: 0,
        bytes: 1,
    }; 3];
    for (i, p) in planes
        .iter_mut()
        .enumerate()
        .take(if layout == Layout::Nv12 { 2 } else { 3 })
    {
        let (w, h) = if i == 0 {
            (width, height)
        } else {
            (width.div_ceil(2), height.div_ceil(2))
        };
        let components = if layout == Layout::Nv12 && i == 1 {
            2
        } else {
            1
        };
        let bytes = components * storage.bytes();
        let stride = f.linesize[i];
        let pointer = f.data[i];
        ensure!(
            !pointer.is_null()
                && stride.unsigned_abs() >= w * bytes
                && stride.unsigned_abs().is_multiple_of(storage.bytes()),
            "Invalid YUV plane stride"
        );
        let delta = (h - 1) as i64 * stride as i64;
        let start = (pointer as usize as i128) + delta.min(0) as i128;
        let end = (pointer as usize as i128) + delta.max(0) as i128 + (w * bytes) as i128;
        let valid = f.buf.iter().copied().filter(|b| !b.is_null()).any(|b| {
            let b = unsafe { &*b };
            start >= b.data as usize as i128 && end <= b.data as usize as i128 + b.size as i128
        });
        ensure!(valid, "YUV plane extends beyond its owning AVBuffer");
        *p = Plane {
            pointer,
            stride,
            width: w,
            height: h,
            bytes,
        };
    }
    Ok(Planes {
        layout,
        storage,
        planes,
        width,
        height,
        matrix,
        color: super::color::profile(frame)?,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn odd_dimensions_and_padded_planes_are_validated() {
        for format in [
            Pixel::NV12,
            Pixel::YUV420P,
            Pixel::P010LE,
            Pixel::YUV420P10LE,
        ] {
            let frame = Video::new(format, 127, 95);
            let p = describe(&frame).unwrap();
            assert_eq!((p.planes[1].width, p.planes[1].height), (64, 48));
        }
    }
    #[test]
    fn rejects_unvalidated_formats_and_invalid_stride() {
        assert!(describe(&Video::new(Pixel::YUV420P12LE, 16, 16)).is_err());
        let mut f = Video::new(Pixel::NV12, 16, 16);
        unsafe {
            (*f.as_mut_ptr()).linesize[0] = 1;
        }
        assert!(describe(&f).is_err());
    }
    #[test]
    fn signed_stride_keeps_plane_inside_owner() {
        for format in [
            Pixel::NV12,
            Pixel::YUV420P,
            Pixel::P010LE,
            Pixel::YUV420P10LE,
        ] {
            let mut f = Video::new(format, 16, 16);
            let planes = describe(&f).unwrap();
            unsafe {
                let p = f.as_mut_ptr();
                for (i, plane) in planes
                    .planes
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| !p.pointer.is_null())
                {
                    (*p).data[i] = (*p).data[i]
                        .offset((*p).linesize[i] as isize * (plane.height - 1) as isize);
                    (*p).linesize[i] = -(*p).linesize[i];
                }
            }
            assert!(describe(&f).is_ok());
        }
    }
    #[test]
    fn ten_bit_endpoints_and_hdr_defaults_use_the_correct_code_range() {
        use ffmpeg::ffi::{AVColorRange::*, AVColorTransferCharacteristic::*};
        for pixel in [Pixel::P010LE, Pixel::YUV420P10LE] {
            for (range, black, white) in
                [(AVCOL_RANGE_MPEG, 64., 940.), (AVCOL_RANGE_JPEG, 0., 1023.)]
            {
                let mut f = Video::new(pixel, 16, 16);
                unsafe {
                    (*f.as_mut_ptr()).color_range = range;
                }
                for row in coefficients(&f).unwrap() {
                    for (y, expected) in [(black, 0.), (white, 1.)] {
                        assert!(
                            (row[0] * y / 1023. + (row[1] + row[2]) * 512. / 1023. + row[3]
                                - expected)
                                .abs()
                                < 1e-6
                        );
                    }
                }
            }
            let mut f = Video::new(pixel, 16, 16);
            unsafe {
                (*f.as_mut_ptr()).color_trc = AVCOL_TRC_SMPTE2084;
            }
            let planes = describe(&f).unwrap();
            assert_eq!(
                planes.color.primaries,
                super::super::color::Primaries::Bt2020
            );
            for row in planes.matrix {
                assert!(
                    (row[0] * 64. / 1023. + (row[1] + row[2]) * 512. / 1023. + row[3]).abs() < 1e-6
                );
            }
            unsafe {
                (*f.as_mut_ptr()).linesize[1] += 1;
            }
            assert!(describe(&f).is_err());
        }
    }
    #[test]
    fn black_white_and_neutral_chroma_are_exact_in_all_sdr_matrices() {
        use ffmpeg::ffi::{AVColorRange::*, AVColorSpace::*};
        for space in [
            AVCOL_SPC_BT709,
            AVCOL_SPC_SMPTE170M,
            AVCOL_SPC_FCC,
            AVCOL_SPC_SMPTE240M,
            AVCOL_SPC_BT2020_NCL,
        ] {
            for range in [AVCOL_RANGE_MPEG, AVCOL_RANGE_JPEG] {
                let mut f = Video::new(Pixel::NV12, 16, 16);
                unsafe {
                    (*f.as_mut_ptr()).colorspace = space;
                    (*f.as_mut_ptr()).color_range = range;
                }
                let matrix = coefficients(&f).unwrap();
                for (y, expected) in if range == AVCOL_RANGE_MPEG {
                    [(16., 0.), (235., 1.)]
                } else {
                    [(0., 0.), (255., 1.)]
                } {
                    for row in matrix {
                        let v = row[0] * y / 255. + (row[1] + row[2]) * 128. / 255. + row[3];
                        assert!((v - expected).abs() < 0.000001);
                    }
                }
            }
        }
    }
}
