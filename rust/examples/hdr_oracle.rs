// SPDX-License-Identifier: MPL-2.0
//! Independent f64 colour oracle: derive primary transforms from xy coordinates,
//! evaluate transfer equations directly, and express the shoulder as a Bezier.
use airdock::render::VideoPipeline;
use anyhow::{Result, ensure};
use ffmpeg_next::{ffi::*, format::Pixel, frame::Video};
use iced_wgpu::{primitive::Pipeline, wgpu};

pub async fn verify(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(usize, u8)> {
    use AVColorPrimaries::*;
    use AVColorRange::*;
    use AVColorSpace::*;
    use AVColorTransferCharacteristic::*;
    let samples = [0u16, 64, 128, 256, 384, 512, 640, 768, 876, 940, 1023]
        .into_iter()
        .flat_map(|y| {
            [0u16, 256, 512, 768, 1023].into_iter().flat_map(move |u| {
                [0u16, 256, 512, 768, 1023]
                    .into_iter()
                    .map(move |v| (y, u, v))
            })
        })
        .collect::<Vec<_>>();
    let width = 256;
    let height = (samples.len() as u32).div_ceil(64) * 4;
    let mut checks = 0;
    let mut worst = 0;
    for srgb in [false, true] {
        let format = if srgb {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let mut pipeline = VideoPipeline::new(device, queue, format);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("10-bit/HDR reference target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (width * height * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut cases = Vec::new();
        for (space, kr, kb) in [
            (AVCOL_SPC_BT709, 0.2126, 0.0722),
            (AVCOL_SPC_SMPTE170M, 0.299, 0.114),
            (AVCOL_SPC_FCC, 0.30, 0.11),
            (AVCOL_SPC_SMPTE240M, 0.212, 0.087),
            (AVCOL_SPC_BT2020_NCL, 0.2627, 0.0593),
        ] {
            cases.push((
                space,
                AVCOL_TRC_UNSPECIFIED,
                AVCOL_PRI_UNSPECIFIED,
                1000.,
                kr,
                kb,
            ));
        }
        for primaries in [AVCOL_PRI_BT709, AVCOL_PRI_BT2020, AVCOL_PRI_SMPTE432] {
            for (trc, peak) in [
                (AVCOL_TRC_SMPTE2084, 400.),
                (AVCOL_TRC_SMPTE2084, 1000.),
                (AVCOL_TRC_SMPTE2084, 4000.),
                (AVCOL_TRC_SMPTE2084, 10000.),
                (AVCOL_TRC_ARIB_STD_B67, 1000.),
            ] {
                cases.push((AVCOL_SPC_BT2020_NCL, trc, primaries, peak, 0.2627, 0.0593));
            }
        }
        cases.push((
            AVCOL_SPC_BT2020_NCL,
            AVCOL_TRC_BT2020_10,
            AVCOL_PRI_BT2020,
            1000.,
            0.2627,
            0.0593,
        ));
        cases.push((
            AVCOL_SPC_BT709,
            AVCOL_TRC_IEC61966_2_1,
            AVCOL_PRI_SMPTE432,
            1000.,
            0.2126,
            0.0722,
        ));
        // One pipeline crosses byte/word layout, colour/peak changes and back.
        for (space, trc, primaries, peak, kr, kb) in cases {
            for limited in [false, true] {
                for pixel in [
                    Pixel::P010LE,
                    Pixel::YUV420P10LE,
                    Pixel::NV12,
                    Pixel::YUV420P,
                ] {
                    let wide = matches!(pixel, Pixel::P010LE | Pixel::YUV420P10LE);
                    let mut frame = Video::new(pixel, width, height);
                    unsafe {
                        let f = frame.as_mut_ptr();
                        (*f).colorspace = space;
                        (*f).color_trc = trc;
                        (*f).color_primaries = primaries;
                        (*f).color_range = if limited {
                            AVCOL_RANGE_MPEG
                        } else {
                            AVCOL_RANGE_JPEG
                        };
                        if trc == AVCOL_TRC_SMPTE2084 {
                            let data = av_frame_new_side_data(
                                f,
                                AVFrameSideDataType::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL,
                                8,
                            );
                            ensure!(!data.is_null(), "Unable to allocate fixture HDR metadata");
                            std::ptr::write_bytes((*data).data, 0, 8);
                            std::ptr::copy_nonoverlapping(
                                (peak as u32).to_ne_bytes().as_ptr(),
                                (*data).data,
                                4,
                            );
                        }
                    }
                    for (i, (y, u, v)) in samples.iter().copied().enumerate() {
                        let x = (i % 64) * 4;
                        let row = (i / 64) * 4;
                        for dy in 0..4 {
                            for dx in 0..4 {
                                write(&mut frame, 0, x + dx, row + dy, y, pixel);
                            }
                        }
                        for dy in 0..2 {
                            for dx in 0..2 {
                                if matches!(pixel, Pixel::P010LE | Pixel::NV12) {
                                    write(&mut frame, 1, x + dx * 2, row / 2 + dy, u, pixel);
                                    write(&mut frame, 1, x + dx * 2 + 1, row / 2 + dy, v, pixel);
                                } else {
                                    write(&mut frame, 1, x / 2 + dx, row / 2 + dy, u, pixel);
                                    write(&mut frame, 2, x / 2 + dx, row / 2 + dy, v, pixel);
                                }
                            }
                        }
                    }
                    pipeline.upload(device, queue, &frame, [1., 1., 0., 0.])?;
                    let view = texture.create_view(&Default::default());
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("Independent HDR oracle"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            ..Default::default()
                        });
                        pass.set_pipeline(pipeline.active_pipeline());
                        pass.set_bind_group(0, pipeline.bind.as_ref().unwrap(), &[]);
                        pass.draw(0..6, 0..1);
                    }
                    encoder.copy_texture_to_buffer(
                        wgpu::TexelCopyTextureInfo {
                            texture: &texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyBufferInfo {
                            buffer: &buffer,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(width * 4),
                                rows_per_image: Some(height),
                            },
                        },
                        wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                    );
                    queue.submit([encoder.finish()]);
                    let (tx, rx) = std::sync::mpsc::channel();
                    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                        let _ = tx.send(r);
                    });
                    device.poll(wgpu::PollType::wait_indefinitely())?;
                    rx.recv()??;
                    let bytes = buffer.slice(..).get_mapped_range();
                    for (i, (y, u, v)) in samples.iter().copied().enumerate() {
                        let maximum = if wide { 1023. } else { 255. };
                        let step = if wide { 4. } else { 1. };
                        let raw = |x: u16| if wide { x as f64 } else { (x >> 2) as f64 };
                        let y = (raw(y) - if limited { 16. * step } else { 0. })
                            / if limited { 219. * step } else { maximum };
                        let u =
                            (raw(u) - 128. * step) / if limited { 224. * step } else { maximum };
                        let v =
                            (raw(v) - 128. * step) / if limited { 224. * step } else { maximum };
                        let r = y + 2. * (1. - kr) * v;
                        let b = y + 2. * (1. - kb) * u;
                        let g = (y - kr * r - kb * b) / (1. - kr - kb);
                        let expected =
                            reference([r, g, b].map(|x| x.clamp(0., 1.)), trc, primaries, peak);
                        let offset = (((i / 64) * 4 + 1) * width as usize + (i % 64) * 4 + 1) * 4;
                        for (channel, value) in expected.into_iter().enumerate() {
                            let code = (value.clamp(0., 1.) * 255.).round() as u8;
                            let error = bytes[offset + channel].abs_diff(code);
                            worst = worst.max(error);
                            ensure!(
                                error <= 2,
                                "10-bit/HDR mismatch: {pixel:?} {trc:?} {primaries:?} peak={peak} limited={limited} srgb={srgb} sample={i} channel={channel} actual={} expected={code}",
                                bytes[offset + channel]
                            );
                        }
                        checks += 1;
                    }
                    drop(bytes);
                    buffer.unmap();
                }
            }
        }
    }
    Ok((checks, worst))
}
fn write(frame: &mut Video, plane: usize, x: usize, y: usize, code: u16, pixel: Pixel) {
    let word = match pixel {
        Pixel::P010LE => Some((code << 6) | 63),
        Pixel::YUV420P10LE => Some(code | 0xfc00),
        _ => None,
    };
    let offset = y * frame.stride(plane) + x * if word.is_some() { 2 } else { 1 };
    if let Some(word) = word {
        frame.data_mut(plane)[offset..offset + 2].copy_from_slice(&word.to_le_bytes());
    } else {
        frame.data_mut(plane)[offset] = (code >> 2) as u8;
    }
}
fn pq(x: f64, inverse: bool) -> f64 {
    let (m1, m2, c1, c2, c3) = (0.1593017578125, 78.84375, 0.8359375, 18.8515625, 18.6875);
    if inverse {
        let z = x.powf(1. / m2);
        10000. * ((z - c1).max(0.) / (c2 - c3 * z)).powf(1. / m1)
    } else {
        let z = (x / 10000.).powf(m1);
        ((c1 + c2 * z) / (1. + c3 * z)).powf(m2)
    }
}
fn shoulder(nits: f64, peak: f64) -> f64 {
    let p = pq(peak, false);
    let end = pq(100., false) / p;
    let start = 1.5 * end - 0.5;
    let x = pq(nits.min(peak), false) / p;
    let y = if x <= start {
        x
    } else {
        let t = (x - start) / (1. - start);
        let u = 1. - t;
        u.powi(3) * start
            + 3. * u * u * t * (start + (1. - start) / 3.)
            + 3. * u * t * t * end
            + t.powi(3) * end
    };
    pq(y * p, true) / 100.
}
fn reference(
    rgb: [f64; 3],
    trc: AVColorTransferCharacteristic,
    primaries: AVColorPrimaries,
    peak: f64,
) -> [f64; 3] {
    use AVColorTransferCharacteristic::*;
    if trc == AVCOL_TRC_UNSPECIFIED {
        return rgb;
    }
    let source = primary_matrix(primaries);
    let target = primary_matrix(AVColorPrimaries::AVCOL_PRI_BT709);
    let mut light = rgb.map(|x| match trc {
        AVCOL_TRC_SMPTE2084 => pq(x, true),
        AVCOL_TRC_ARIB_STD_B67 => {
            if x <= 0.5 {
                x * x / 3.
            } else {
                (((x - 0.55991073) / 0.17883277).exp() + 0.28466892) / 12.
            }
        }
        AVCOL_TRC_IEC61966_2_1 => {
            if x <= 0.04045 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            }
        }
        _ => {
            if x < 0.081 {
                x / 4.5
            } else {
                ((x + 0.099) / 1.099).powf(1. / 0.45)
            }
        }
    });
    if matches!(trc, AVCOL_TRC_SMPTE2084 | AVCOL_TRC_ARIB_STD_B67) {
        let mut lum = dot(source[1], light);
        if trc == AVCOL_TRC_ARIB_STD_B67 {
            light = light.map(|x| x * 1000. * lum.powf(0.2));
            lum = dot(source[1], light);
        }
        let ratio = if lum > 1e-6 {
            shoulder(lum, peak) / lum
        } else {
            0.
        };
        light = light.map(|x| x * ratio);
    }
    let xyz = source.map(|row| dot(row, light));
    let inverse = invert(target);
    let converted = inverse.map(|row| dot(row, xyz));
    let grey = dot([0.2126, 0.7152, 0.0722], converted).clamp(0., 1.);
    let delta = converted.map(|x| x - grey);
    let mut factor: f64 = 1.;
    for d in delta {
        if d > 1e-6 {
            factor = factor.min((1. - grey) / d);
        }
        if d < -1e-6 {
            factor = factor.min(-grey / d);
        }
    }
    delta.map(|d| {
        let x = (grey + factor * d).clamp(0., 1.);
        if x <= 0.0031308 {
            x * 12.92
        } else {
            1.055 * x.powf(1. / 2.4) - 0.055
        }
    })
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
fn primary_matrix(primaries: AVColorPrimaries) -> [[f64; 3]; 3] {
    use AVColorPrimaries::*;
    let xy = match primaries {
        AVCOL_PRI_BT2020 => [(0.708, 0.292), (0.170, 0.797), (0.131, 0.046)],
        AVCOL_PRI_SMPTE432 => [(0.680, 0.320), (0.265, 0.690), (0.150, 0.060)],
        _ => [(0.640, 0.330), (0.300, 0.600), (0.150, 0.060)],
    };
    let base = std::array::from_fn(|r| {
        std::array::from_fn(|c| match r {
            0 => xy[c].0 / xy[c].1,
            1 => 1.,
            _ => (1. - xy[c].0 - xy[c].1) / xy[c].1,
        })
    });
    let weights =
        invert(base).map(|row| dot(row, [0.3127 / 0.3290, 1., (1. - 0.3127 - 0.3290) / 0.3290]));
    std::array::from_fn(|r| std::array::from_fn(|c| base[r][c] * weights[c]))
}
fn invert(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let cofactor = std::array::from_fn::<_, 3, _>(|r| {
        std::array::from_fn::<_, 3, _>(|c| {
            let rows = (0..3).filter(|i| *i != r).collect::<Vec<_>>();
            let cols = (0..3).filter(|i| *i != c).collect::<Vec<_>>();
            (m[rows[0]][cols[0]] * m[rows[1]][cols[1]] - m[rows[0]][cols[1]] * m[rows[1]][cols[0]])
                * if (r + c) % 2 == 0 { 1. } else { -1. }
        })
    });
    let determinant = dot(m[0], cofactor[0]);
    std::array::from_fn(|r| std::array::from_fn(|c| cofactor[c][r] / determinant))
}
