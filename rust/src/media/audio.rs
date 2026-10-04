// SPDX-License-Identifier: MPL-2.0
use anyhow::{Result, bail, ensure};
use ffmpeg_next::{
    self as ffmpeg, ChannelLayout, codec,
    format::{Sample, sample::Type},
    frame,
};
use std::{ptr, sync::Arc};

pub fn eld_config(rate: u32, channels: u8, spf: u32) -> Result<Vec<u8>> {
    let index = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ]
    .iter()
    .position(|r| *r == rate)
    .ok_or_else(|| anyhow::anyhow!("Unsupported AAC sample rate"))?;
    ensure!(matches!(channels, 1 | 2), "Unsupported channel count");
    ensure!(
        matches!(spf, 480 | 512),
        "AAC-ELD frame length must be 480 or 512"
    );
    let mut bytes = vec![0; 4];
    let mut position = 0;
    for (value, bits) in [
        (31, 5),
        (7, 6),
        (index as u32, 4),
        (channels as u32, 4),
        (u32::from(spf == 480), 1),
        (0, 4),
        (0, 4),
    ] {
        for i in (0..bits).rev() {
            bytes[position / 8] |= ((value >> i & 1) as u8) << (7 - position % 8);
            position += 1;
        }
    }
    Ok(bytes)
}
pub fn alac_config(rate: u32, channels: u8, spf: u32) -> Vec<u8> {
    let mut bytes = vec![0, 0, 0, 36, b'a', b'l', b'a', b'c', 0, 0, 0, 0];
    bytes.extend_from_slice(&spf.to_be_bytes());
    bytes.extend_from_slice(&[0, 16, 40, 10, 14, channels, 0, 255]);
    bytes.extend_from_slice(&[0; 8]);
    bytes.extend_from_slice(&rate.to_be_bytes());
    bytes
}

pub fn extradata(context: &mut codec::Context, bytes: &[u8]) -> Result<()> {
    unsafe {
        let ctx = context.as_mut_ptr();
        ensure!((*ctx).extradata.is_null(), "Extradata already installed");
        let pointer = ffmpeg::ffi::av_mallocz(
            bytes.len() + ffmpeg::ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize,
        ) as *mut u8;
        ensure!(!pointer.is_null(), "Unable to allocate codec configuration");
        ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
        (*ctx).extradata = pointer;
        (*ctx).extradata_size = bytes.len() as i32;
    }
    Ok(())
}

pub struct Decoder {
    decoder: codec::decoder::Audio,
    resampler: Option<ffmpeg::software::resampling::Context>,
    rate: u32,
    channels: u8,
}
impl Decoder {
    pub fn new(ct: u64, rate: u32, channels: u8, spf: u32) -> Result<Self> {
        ensure!(
            matches!(channels, 1 | 2),
            "Only mono and stereo audio are supported"
        );
        let id = match ct {
            2 => codec::Id::ALAC,
            3 | 4 | 8 => codec::Id::AAC,
            _ => bail!("Unsupported AirPlay audio codec {ct}"),
        };
        let codec =
            codec::decoder::find(id).ok_or_else(|| anyhow::anyhow!("Audio decoder missing"))?;
        let mut context = codec::Context::new_with_codec(codec);
        let config = if ct == 2 {
            alac_config(rate, channels, if spf == 0 { 352 } else { spf })
        } else if ct == 3 {
            let i = [
                96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000,
                7350,
            ]
            .iter()
            .position(|r| *r == rate)
            .ok_or_else(|| anyhow::anyhow!("Unsupported AAC rate"))?;
            vec![
                (2 << 3) | (i as u8 >> 1),
                ((i as u8 & 1) << 7) | (channels << 3),
            ]
        } else {
            eld_config(rate, channels, spf)?
        };
        extradata(&mut context, &config)?;
        unsafe {
            let c = context.as_mut_ptr();
            (*c).sample_rate = rate as i32;
            ffmpeg::ffi::av_channel_layout_default(&mut (*c).ch_layout, channels as i32);
        }
        let decoder = context.decoder().open_as(codec)?.audio()?;
        Ok(Self {
            decoder,
            resampler: None,
            rate,
            channels,
        })
    }
    pub fn flush(&mut self) {
        self.decoder.flush();
        self.resampler = None;
    }
    pub fn decode(&mut self, bytes: &[u8]) -> Result<Arc<Vec<i16>>> {
        self.decoder.send_packet(&ffmpeg::Packet::copy(bytes))?;
        let mut pcm = Vec::new();
        let mut frame = frame::Audio::empty();
        loop {
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => {}
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
                Err(ffmpeg::Error::Eof) => break,
                Err(e) => return Err(e.into()),
            }
            if frame.channel_layout().is_empty() {
                frame.set_channel_layout(if self.channels == 2 {
                    ChannelLayout::STEREO
                } else {
                    ChannelLayout::MONO
                });
            }
            if frame.rate() == 0 {
                frame.set_rate(self.rate);
            }
            let definition = ffmpeg::software::resampling::context::Definition {
                format: frame.format(),
                channel_layout: frame.channel_layout(),
                rate: frame.rate(),
            };
            if self
                .resampler
                .as_ref()
                .is_none_or(|r| r.input() != &definition)
            {
                self.resampler = Some(ffmpeg::software::resampling::Context::get(
                    frame.format(),
                    frame.channel_layout(),
                    frame.rate(),
                    Sample::I16(Type::Packed),
                    if self.channels == 2 {
                        ChannelLayout::STEREO
                    } else {
                        ChannelLayout::MONO
                    },
                    self.rate,
                )?);
            }
            let mut output = frame::Audio::empty();
            self.resampler.as_mut().unwrap().run(&frame, &mut output)?;
            let count = output.samples() * self.channels as usize;
            let values = unsafe {
                std::slice::from_raw_parts((*output.as_ptr()).data[0] as *const i16, count)
            };
            pcm.extend_from_slice(values);
        }
        Ok(Arc::new(pcm))
    }
}

pub use crate::audio::Sink;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eld_480_and_512_config_have_the_correct_frame_flag() {
        assert_eq!(eld_config(44100, 2, 480).unwrap(), [0xf8, 0xe8, 0x50, 0]);
        assert_eq!(eld_config(44100, 2, 512).unwrap(), [0xf8, 0xe8, 0x40, 0]);
        assert!(eld_config(44100, 2, 352).is_err());
    }
    #[test]
    fn alac_cookie_matches_apple_layout() {
        let c = alac_config(44100, 2, 352);
        assert_eq!(c.len(), 36);
        assert_eq!(&c[12..16], &352u32.to_be_bytes());
        assert_eq!(&c[32..], &44100u32.to_be_bytes());
    }
}
