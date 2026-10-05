// SPDX-License-Identifier: MPL-2.0
//! Offscreen checks execute the production YUV shader, including sRGB target handling.
use airdock::render::VideoPipeline;
use anyhow::{Result, ensure};
use ffmpeg_next::{
    ffi::{AVColorRange::*, AVColorSpace::*},
    format::Pixel,
    frame::Video,
};
use iced_wgpu::{primitive::Pipeline, wgpu};
mod hdr_oracle;
fn main() -> Result<()> {
    futures::executor::block_on(verify())
}
async fn verify() -> Result<()> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::all()),
        ..Default::default()
    });
    let adapter = match instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            force_fallback_adapter: true,
            ..Default::default()
        })
        .await
    {
        Ok(a) => a,
        Err(_) => instance.request_adapter(&Default::default()).await?,
    };
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await?;
    let samples = [0u8, 16, 32, 64, 128, 192, 235, 255]
        .into_iter()
        .flat_map(|y| {
            [0u8, 16, 64, 128, 192, 240, 255]
                .into_iter()
                .flat_map(move |u| {
                    [0u8, 16, 64, 128, 192, 240, 255]
                        .into_iter()
                        .map(move |v| (y, u, v))
                })
        })
        .collect::<Vec<_>>();
    let (width, height) = (256u32, (samples.len() as u32).div_ceil(64) * 4);
    let mut checks = 0;
    let mut worst = 0u8;
    for (space, kr, kb) in [
        (AVCOL_SPC_UNSPECIFIED, 0.2126, 0.0722),
        (AVCOL_SPC_BT709, 0.2126, 0.0722),
        (AVCOL_SPC_SMPTE170M, 0.299, 0.114),
        (AVCOL_SPC_FCC, 0.30, 0.11),
        (AVCOL_SPC_SMPTE240M, 0.212, 0.087),
        (AVCOL_SPC_BT2020_NCL, 0.2627, 0.0593),
    ] {
        for limited in [false, true] {
            for pixel in [Pixel::NV12, Pixel::YUV420P] {
                for srgb in [false, true] {
                    let mut frame = Video::new(pixel, width, height);
                    unsafe {
                        (*frame.as_mut_ptr()).colorspace = space;
                        (*frame.as_mut_ptr()).color_range = if limited {
                            AVCOL_RANGE_MPEG
                        } else {
                            AVCOL_RANGE_JPEG
                        };
                    }
                    for (i, (y, u, v)) in samples.iter().copied().enumerate() {
                        let (x, row) = ((i % 64) * 4, (i / 64) * 4);
                        let stride = frame.stride(0);
                        for dy in 0..4 {
                            frame.data_mut(0)[(row + dy) * stride + x..(row + dy) * stride + x + 4]
                                .fill(y);
                        }
                        for dy in 0..2 {
                            let offset = (row / 2 + dy) * frame.stride(1);
                            if pixel == Pixel::NV12 {
                                for dx in 0..2 {
                                    frame.data_mut(1)[offset + x + dx * 2..offset + x + dx * 2 + 2]
                                        .copy_from_slice(&[u, v]);
                                }
                            } else {
                                frame.data_mut(1)[offset + x / 2..offset + x / 2 + 2].fill(u);
                                let offset = (row / 2 + dy) * frame.stride(2);
                                frame.data_mut(2)[offset + x / 2..offset + x / 2 + 2].fill(v);
                            }
                        }
                    }
                    let format = if srgb {
                        wgpu::TextureFormat::Rgba8UnormSrgb
                    } else {
                        wgpu::TextureFormat::Rgba8Unorm
                    };
                    let mut pipeline = VideoPipeline::new(&device, &queue, format);
                    pipeline.upload(&device, &queue, &frame, [1., 1., 0., 0.])?;
                    let texture = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("YUV oracle target"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    });
                    let view = texture.create_view(&Default::default());
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: None,
                        size: (width * height * 4) as u64,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("Production YUV shader oracle"),
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
                        pass.set_pipeline(&pipeline.pipeline);
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
                    buffer
                        .slice(..)
                        .map_async(wgpu::MapMode::Read, move |result| {
                            let _ = tx.send(result);
                        });
                    device.poll(wgpu::PollType::wait_indefinitely())?;
                    rx.recv()??;
                    let bytes = buffer.slice(..).get_mapped_range();
                    for (i, (y, u, v)) in samples.iter().copied().enumerate() {
                        // Independent f64 scalar oracle; does not call production coefficient generation.
                        let y = (y as f64 - if limited { 16. } else { 0. })
                            / if limited { 219. } else { 255. };
                        let u = (u as f64 - 128.) / if limited { 224. } else { 255. };
                        let v = (v as f64 - 128.) / if limited { 224. } else { 255. };
                        let r = y + 2. * (1. - kr) * v;
                        let b = y + 2. * (1. - kb) * u;
                        let g = (y - kr * r - kb * b) / (1. - kr - kb);
                        let offset = (((i / 64) * 4 + 1) * width as usize + (i % 64) * 4 + 1) * 4;
                        for (channel, value) in [r, g, b].into_iter().enumerate() {
                            let expected = (value.clamp(0., 1.) * 255.).round() as u8;
                            let error = bytes[offset + channel].abs_diff(expected);
                            worst = worst.max(error);
                            ensure!(
                                error <= 2,
                                "Shader mismatch: space={space:?} limited={limited} format={pixel:?} srgb={srgb} sample={i} channel={channel} actual={} expected={expected}",
                                bytes[offset + channel]
                            );
                        }
                        checks += 1;
                    }
                }
            }
        }
    }
    let (hdr_checks, hdr_worst) = hdr_oracle::verify(&device, &queue).await?;
    println!(
        "{}",
        serde_json::json!({"adapter":adapter.get_info().name,"backend":format!("{:?}",adapter.get_info().backend),"checks":checks+hdr_checks,"sdr_checks":checks,"ten_bit_hdr_checks":hdr_checks,"max_error_255":worst.max(hdr_worst),"hardware_performance_verified":false})
    );
    Ok(())
}
