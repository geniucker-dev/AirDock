// SPDX-License-Identifier: MPL-2.0
//! Display-referred HDR to SDR, using ST 2084 / BT.2100 and a BT.2390 EETF.
//! Tables are generated only when colour metadata changes, never per video pixel.
use anyhow::{Result, bail};
use ffmpeg_next::{self as ffmpeg, frame::Video};

pub const LUT_SIZE: usize = 4096;
pub const SDR_PEAK: f64 = 100.;

// ffmpeg-sys omits mastering_display_metadata.h. Public prefix layouts match
// pinned FFmpeg 8.1 and development FFmpeg 7. Payload sizes are checked below.
#[repr(C)]
struct MasteringMetadata {
    primaries: [[ffmpeg::ffi::AVRational; 2]; 3],
    white_point: [ffmpeg::ffi::AVRational; 2],
    min_luminance: ffmpeg::ffi::AVRational,
    max_luminance: ffmpeg::ffi::AVRational,
    has_primaries: i32,
    has_luminance: i32,
}
#[repr(C)]
struct LightMetadata {
    max_cll: u32,
    max_fall: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    Identity,
    Pq,
    Hlg,
    Bt709,
    Srgb,
    Gamma22,
    Gamma28,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primaries {
    Bt709,
    Bt2020,
    DisplayP3,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    pub transfer: Transfer,
    pub primaries: Primaries,
    pub peak: f32,
}
impl Profile {
    pub fn mode(self) -> f32 {
        match self.transfer {
            Transfer::Identity => 0.,
            Transfer::Pq => 1.,
            Transfer::Hlg => 2.,
            _ => 3.,
        }
    }
    pub fn gamut(self) -> [[f32; 4]; 3] {
        // Linear-light D65 transforms. Row .w carries source luminance weights.
        match self.primaries {
            Primaries::Bt709 => [
                [1., 0., 0., 0.2126],
                [0., 1., 0., 0.7152],
                [0., 0., 1., 0.0722],
            ],
            Primaries::Bt2020 => [
                [1.660491, -0.587641, -0.072850, 0.2627],
                [-0.124550, 1.132_9, -0.008350, 0.6780],
                [-0.018151, -0.100579, 1.118_73, 0.0593],
            ],
            Primaries::DisplayP3 => [
                [1.224_94, -0.224940, 0., 0.228975],
                [-0.042057, 1.042057, 0., 0.691739],
                [-0.019638, -0.078636, 1.098274, 0.079286],
            ],
        }
    }
    pub fn tables(self) -> Vec<f32> {
        let mut data = Vec::with_capacity(LUT_SIZE * 2);
        for i in 0..LUT_SIZE {
            let x = i as f64 / (LUT_SIZE - 1) as f64;
            data.push(decode(self.transfer, x) as f32);
        }
        for i in 0..LUT_SIZE {
            let nits = self.peak as f64 * i as f64 / (LUT_SIZE - 1) as f64;
            data.push((tone_map(nits, self.peak as f64) / SDR_PEAK) as f32);
        }
        data
    }
}

pub fn profile(frame: &Video) -> Result<Profile> {
    use ffmpeg::ffi::{AVColorPrimaries::*, AVColorSpace::*, AVColorTransferCharacteristic::*};
    let f = unsafe { &*frame.as_ptr() };
    let hdr = matches!(f.color_trc, AVCOL_TRC_SMPTE2084 | AVCOL_TRC_ARIB_STD_B67);
    let primaries = match f.color_primaries {
        AVCOL_PRI_BT2020 => Primaries::Bt2020,
        AVCOL_PRI_SMPTE432 => Primaries::DisplayP3,
        AVCOL_PRI_BT709 => Primaries::Bt709,
        AVCOL_PRI_UNSPECIFIED if hdr => {
            if f.colorspace == AVCOL_SPC_BT709 {
                Primaries::Bt709
            } else {
                Primaries::Bt2020
            }
        }
        // Preserve the existing 8-bit SDR matrix path and its unspecified metadata.
        _ if !hdr => Primaries::Bt709,
        other => bail!("Unsupported HDR colour primaries {other:?}"),
    };
    let transfer = match f.color_trc {
        AVCOL_TRC_SMPTE2084 => Transfer::Pq,
        AVCOL_TRC_ARIB_STD_B67 => Transfer::Hlg,
        _ if primaries == Primaries::Bt709 => Transfer::Identity,
        AVCOL_TRC_IEC61966_2_1 => Transfer::Srgb,
        AVCOL_TRC_GAMMA22 => Transfer::Gamma22,
        AVCOL_TRC_GAMMA28 => Transfer::Gamma28,
        AVCOL_TRC_BT709
        | AVCOL_TRC_BT2020_10
        | AVCOL_TRC_BT2020_12
        | AVCOL_TRC_SMPTE170M
        | AVCOL_TRC_UNSPECIFIED => Transfer::Bt709,
        other => bail!("Unsupported wide-gamut SDR transfer {other:?}"),
    };
    Ok(Profile {
        transfer,
        primaries,
        // BT.2100 nominal HLG display: 1000 nits, system gamma 1.2.
        peak: if transfer == Transfer::Pq {
            source_peak(frame)
        } else {
            1000.
        },
    })
}

fn source_peak(frame: &Video) -> f32 {
    use ffmpeg::ffi::{self, AVFrameSideDataType::*};
    unsafe {
        let light = ffi::av_frame_get_side_data(frame.as_ptr(), AV_FRAME_DATA_CONTENT_LIGHT_LEVEL);
        if !light.is_null()
            && !(*light).data.is_null()
            && (*light).size >= std::mem::size_of::<LightMetadata>()
        {
            let max_cll = std::ptr::read_unaligned((*light).data.cast::<u32>());
            if (1..=10_000).contains(&max_cll) {
                return (max_cll as f32).max(SDR_PEAK as f32);
            }
        }
        let mastering =
            ffi::av_frame_get_side_data(frame.as_ptr(), AV_FRAME_DATA_MASTERING_DISPLAY_METADATA);
        if !mastering.is_null()
            && !(*mastering).data.is_null()
            && (*mastering).size >= std::mem::size_of::<MasteringMetadata>()
        {
            let has_luminance = std::ptr::read_unaligned(
                (*mastering)
                    .data
                    .add(std::mem::offset_of!(MasteringMetadata, has_luminance))
                    .cast::<i32>(),
            );
            if has_luminance == 0 {
                return 1000.;
            }
            let peak = std::ptr::read_unaligned(
                (*mastering)
                    .data
                    .add(std::mem::offset_of!(MasteringMetadata, max_luminance))
                    .cast::<ffi::AVRational>(),
            );
            if peak.num > 0 && peak.den > 0 {
                let nits = peak.num as f32 / peak.den as f32;
                if nits <= 10_000. {
                    return nits.max(SDR_PEAK as f32);
                }
            }
        }
    }
    1000.
}

pub fn pq_decode(signal: f64) -> f64 {
    let p = signal.clamp(0., 1.).powf(32. / 2523.);
    let n = (p - 3424. / 4096.).max(0.);
    let d = (2413. / 128. - 2392. / 128. * p).max(1e-12);
    10_000. * (n / d).powf(16384. / 2610.)
}
fn pq_encode(nits: f64) -> f64 {
    let x = (nits.clamp(0., 10_000.) / 10_000.).powf(2610. / 16384.);
    ((3424. / 4096. + 2413. / 128. * x) / (1. + 2392. / 128. * x)).powf(2523. / 32.)
}
fn decode(transfer: Transfer, x: f64) -> f64 {
    match transfer {
        Transfer::Pq => pq_decode(x),
        Transfer::Hlg => {
            if x <= 0.5 {
                x * x / 3.
            } else {
                ((x - 0.55991073) / 0.17883277)
                    .exp()
                    .mul_add(1., 0.28466892)
                    / 12.
            }
        }
        Transfer::Bt709 => {
            if x < 0.081 {
                x / 4.5
            } else {
                ((x + 0.099) / 1.099).powf(1. / 0.45)
            }
        }
        Transfer::Srgb => {
            if x <= 0.04045 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            }
        }
        Transfer::Gamma22 => x.powf(2.2),
        Transfer::Gamma28 => x.powf(2.8),
        Transfer::Identity => x,
    }
}

/// Zero-black BT.2390 Hermite shoulder in normalized PQ space.
pub fn tone_map(nits: f64, source_peak: f64) -> f64 {
    if source_peak <= SDR_PEAK {
        return nits.clamp(0., SDR_PEAK);
    }
    let peak_pq = pq_encode(source_peak);
    let target = pq_encode(SDR_PEAK) / peak_pq;
    let knee = 1.5 * target - 0.5;
    let input = pq_encode(nits.min(source_peak)) / peak_pq;
    let output = if input <= knee {
        input
    } else {
        let t = (input - knee) / (1. - knee);
        let t2 = t * t;
        let t3 = t2 * t;
        (2. * t3 - 3. * t2 + 1.) * knee
            + (t3 - 2. * t2 + t) * (1. - knee)
            + (-2. * t3 + 3. * t2) * target
    };
    pq_decode(output * peak_pq).clamp(0., SDR_PEAK)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_transfer_anchors_and_monotonic_highlight_rolloff() {
        assert!(pq_decode(0.) < 1e-8);
        assert!((pq_decode(0.5080784215) - 100.).abs() < 1e-5);
        assert!((pq_decode(0.7518270962) - 1000.).abs() < 1e-4);
        assert!((pq_decode(1.) - 10_000.).abs() < 1e-6);
        assert!((decode(Transfer::Hlg, 0.5) - 1. / 12.).abs() < 1e-9);
        assert!((decode(Transfer::Hlg, 1.) - 1.).abs() < 1e-6);
        for peak in [100., 400., 1000., 4000., 10_000.] {
            let mut previous = -1.;
            for i in 0..=1000 {
                let y = tone_map(peak * i as f64 / 1000., peak);
                assert!(y.is_finite() && y >= previous - 1e-9 && y <= 100.);
                previous = y;
            }
            assert!((previous - 100.).abs() < 1e-6);
        }
    }
    #[test]
    fn pq_metadata_selects_cll_then_mastering_and_ignores_invalid_values() {
        use ffmpeg::ffi::*;
        let mut frame = Video::new(ffmpeg::format::Pixel::P010LE, 16, 16);
        unsafe {
            (*frame.as_mut_ptr()).color_trc = AVColorTransferCharacteristic::AVCOL_TRC_SMPTE2084;
            (*frame.as_mut_ptr()).color_primaries = AVColorPrimaries::AVCOL_PRI_BT2020;
            let master_data = av_frame_new_side_data(
                frame.as_mut_ptr(),
                AVFrameSideDataType::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA,
                std::mem::size_of::<MasteringMetadata>(),
            );
            assert!(!master_data.is_null());
            std::ptr::write_bytes(
                (*master_data).data,
                0,
                std::mem::size_of::<MasteringMetadata>(),
            );
            let master = (*master_data).data.cast::<MasteringMetadata>();
            assert!(!master.is_null());
            (*master).has_luminance = 1;
            (*master).max_luminance = AVRational { num: 4000, den: 1 };
            assert_eq!(profile(&frame).unwrap().peak, 4000.);
            let light_data = av_frame_new_side_data(
                frame.as_mut_ptr(),
                AVFrameSideDataType::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL,
                std::mem::size_of::<LightMetadata>(),
            );
            assert!(!light_data.is_null());
            std::ptr::write_bytes((*light_data).data, 0, std::mem::size_of::<LightMetadata>());
            let light = (*light_data).data.cast::<LightMetadata>();
            assert!(!light.is_null());
            (*light).max_cll = 800;
            assert_eq!(profile(&frame).unwrap().peak, 800.);
            (*light).max_cll = 100_000;
            assert_eq!(profile(&frame).unwrap().peak, 4000.);
            (*master).max_luminance.den = 0;
            assert_eq!(profile(&frame).unwrap().peak, 1000.);
        }
    }
    #[test]
    fn hdr_side_data_survives_frame_property_copy_and_short_metadata_is_ignored() {
        use ffmpeg::ffi::*;
        let mut source = Video::new(ffmpeg::format::Pixel::P010LE, 16, 16);
        let mut copied = Video::new(ffmpeg::format::Pixel::P010LE, 16, 16);
        unsafe {
            (*source.as_mut_ptr()).color_trc = AVColorTransferCharacteristic::AVCOL_TRC_SMPTE2084;
            let data = av_frame_new_side_data(
                source.as_mut_ptr(),
                AVFrameSideDataType::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL,
                8,
            );
            assert!(!data.is_null());
            std::ptr::write_bytes((*data).data, 0, 8);
            (*((*data).data.cast::<LightMetadata>())).max_cll = 600;
            assert_eq!(av_frame_copy_props(copied.as_mut_ptr(), source.as_ptr()), 0);
            assert_eq!(profile(&copied).unwrap().peak, 600.);
            (*data).size = 1;
            assert_eq!(profile(&source).unwrap().peak, 1000.);
            assert_eq!(profile(&copied).unwrap().peak, 600.);
        }
    }
}
