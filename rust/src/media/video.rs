use super::audio::extradata;
use anyhow::{Context, Result, bail, ensure};
use ffmpeg_next::{self as ffmpeg, codec, frame};

#[derive(Clone, PartialEq, Eq)]
pub struct Configuration {
    pub hevc: bool,
    pub bytes: Vec<u8>,
    pub length_size: usize,
}
impl Configuration {
    pub fn parse(input: &[u8]) -> Result<Self> {
        ensure!(input.len() <= 2_000_000, "Video configuration too large");
        let (bytes, hint) = if input.first() == Some(&1) {
            (input, None)
        } else {
            let (index, size) = input
                .windows(4)
                .enumerate()
                .filter(|(_, w)| *w == b"avcC" || *w == b"hvcC")
                .find_map(|(index, _)| {
                    if index < 4 {
                        return None;
                    }
                    let size =
                        u32::from_be_bytes(input[index - 4..index].try_into().ok()?) as usize;
                    (size >= 9 && index - 4 + size <= input.len()).then_some((index, size))
                })
                .context("No valid AVC/HEVC configuration box")?;
            (
                &input[index + 4..index - 4 + size],
                Some(&input[index..index + 4] == b"hvcC"),
            )
        };
        ensure!(
            bytes.first() == Some(&1),
            "Invalid AVC/HEVC configuration version"
        );
        let avc = parameter_sets(bytes, false).ok();
        let hevc = hint.unwrap_or(avc.is_none());
        let offset = if hevc { 21 } else { 4 };
        ensure!(bytes.len() > offset + 1, "Truncated codec configuration");
        let length_size = (bytes[offset] & 3) as usize + 1;
        ensure!(matches!(length_size, 1 | 2 | 4), "Invalid NAL length size");
        let bytes = if hevc {
            parameter_sets(bytes, true)?
        } else {
            avc.context("Invalid AVC parameter sets")?
        };
        Ok(Self {
            hevc,
            bytes,
            length_size,
        })
    }
}
fn parameter_sets(bytes: &[u8], hevc: bool) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut types = Vec::new();
    let mut read_nals = |mut offset: usize, count: usize, kind: u8| -> Result<usize> {
        for _ in 0..count {
            let field = bytes
                .get(offset..offset + 2)
                .context("Truncated parameter set length")?;
            offset += 2;
            let length = u16::from_be_bytes(field.try_into()?) as usize;
            let nal = bytes
                .get(offset..offset + length)
                .context("Truncated parameter set")?;
            ensure!(!nal.is_empty(), "Empty parameter set");
            let actual = if hevc {
                (nal[0] >> 1) & 63
            } else {
                nal[0] & 31
            };
            ensure!(actual == kind, "Wrong parameter set NAL type");
            output.extend_from_slice(&[0, 0, 0, 1]);
            output.extend_from_slice(nal);
            types.push(kind);
            offset += length;
        }
        Ok(offset)
    };
    if hevc {
        ensure!(bytes.len() >= 23, "Truncated HEVC configuration");
        let mut offset = 23;
        for _ in 0..bytes[22] {
            let field = bytes
                .get(offset..offset + 3)
                .context("Truncated HEVC array")?;
            let kind = field[0] & 63;
            let count = u16::from_be_bytes(field[1..3].try_into()?) as usize;
            offset = read_nals(offset + 3, count, kind)?;
        }
        ensure!(
            [32, 33, 34].iter().all(|t| types.contains(t)),
            "Missing HEVC VPS/SPS/PPS"
        );
    } else {
        ensure!(bytes.len() >= 7, "Truncated AVC configuration");
        let offset = read_nals(6, (bytes[5] & 31) as usize, 7)?;
        let count = *bytes.get(offset).context("Missing PPS count")?;
        read_nals(offset + 1, count as usize, 8)?;
        ensure!(
            [7, 8].iter().all(|t| types.contains(t)),
            "Missing AVC SPS/PPS"
        );
    }
    Ok(output)
}
pub fn annex_b(bytes: &mut Vec<u8>, length_size: usize) -> Result<()> {
    ensure!(matches!(length_size, 1 | 2 | 4), "Invalid NAL length size");
    if length_size == 4 {
        let mut offset = 0;
        while offset < bytes.len() {
            let field = bytes
                .get(offset..offset + 4)
                .ok_or_else(|| anyhow::anyhow!("Truncated NAL length"))?;
            let len = u32::from_be_bytes(field.try_into()?) as usize;
            ensure!(
                len > 0 && len <= bytes.len() - offset - 4,
                "Truncated NAL unit"
            );
            bytes[offset..offset + 4].copy_from_slice(&[0, 0, 0, 1]);
            offset += 4 + len;
        }
    } else {
        let mut output = Vec::with_capacity(bytes.len() + 32);
        let mut offset = 0;
        while offset < bytes.len() {
            let field = bytes
                .get(offset..offset + length_size)
                .ok_or_else(|| anyhow::anyhow!("Truncated NAL length"))?;
            let len = field.iter().fold(0usize, |v, b| (v << 8) | *b as usize);
            offset += length_size;
            ensure!(len > 0 && len <= bytes.len() - offset, "Truncated NAL unit");
            output.extend_from_slice(&[0, 0, 0, 1]);
            output.extend_from_slice(&bytes[offset..offset + len]);
            offset += len;
        }
        *bytes = output;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    Cpu,
    #[cfg(windows)]
    Nvdec,
    #[cfg(windows)]
    D3d11,
}
impl Backend {
    fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            #[cfg(windows)]
            Self::Nvdec => "NVDEC",
            #[cfg(windows)]
            Self::D3d11 => "D3D11VA",
        }
    }
}

pub struct Decoder {
    inner: codec::decoder::Video,
    pub length_size: usize,
    pub hardware: bool,
    configuration: Configuration,
    backend: Backend,
    requested_hardware: bool,
    pending_configuration: bool,
    scratch: Vec<u8>,
}
impl Decoder {
    pub fn new(config: Configuration, hardware: bool) -> Result<Self> {
        #[cfg(windows)]
        if hardware {
            // Prefer D3D11VA on the rendering adapter to reduce hybrid-GPU
            // traffic and power. NVDEC is retained as the next decode backend.
            for backend in [Backend::D3d11, Backend::Nvdec] {
                match Self::open(config.clone(), backend) {
                    Ok(mut decoder) => {
                        decoder.requested_hardware = hardware;
                        return Ok(decoder);
                    }
                    Err(e) => tracing::warn!("{} unavailable: {e:#}", backend.label()),
                }
            }
        }
        let mut decoder = Self::open(config, Backend::Cpu)?;
        decoder.requested_hardware = hardware;
        Ok(decoder)
    }
    pub fn backend(&self) -> &'static str {
        self.backend.label()
    }
    fn open(config: Configuration, backend: Backend) -> Result<Self> {
        let id = if config.hevc {
            codec::Id::HEVC
        } else {
            codec::Id::H264
        };
        #[cfg(windows)]
        let codec = if backend == Backend::Nvdec {
            codec::decoder::find_by_name(if config.hevc {
                "hevc_cuvid"
            } else {
                "h264_cuvid"
            })
        } else {
            codec::decoder::find(id)
        };
        #[cfg(not(windows))]
        let codec = codec::decoder::find(id);
        let codec = codec.ok_or_else(|| anyhow::anyhow!("Video decoder missing"))?;
        let mut context = codec::Context::new_with_codec(codec);
        extradata(&mut context, &config.bytes)?;
        context.set_threading(codec::threading::Config::kind(
            codec::threading::Type::Slice,
        ));
        context.set_flags(codec::Flags::LOW_DELAY);
        unsafe {
            (*context.as_mut_ptr()).pkt_timebase = ffmpeg::ffi::AVRational {
                num: 1,
                den: 1_000_000,
            };
            (*context.as_mut_ptr()).extra_hw_frames = 1;
            (*context.as_mut_ptr()).max_pixels = 8192 * 8192;
        }
        #[cfg(windows)]
        if backend != Backend::Cpu {
            unsafe {
                let kind = if backend == Backend::Nvdec {
                    ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA
                } else {
                    ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA
                };
                let adapter = if backend == Backend::D3d11 {
                    crate::platform::decoder_adapter(
                        crate::render::compositor::VENDOR
                            .load(std::sync::atomic::Ordering::Acquire),
                        crate::render::compositor::DEVICE
                            .load(std::sync::atomic::Ordering::Acquire),
                    )
                    .and_then(|s| std::ffi::CString::new(s).ok())
                } else {
                    None
                };
                if let Some(index) = &adapter {
                    tracing::info!(
                        "D3D11VA render-matched DXGI adapter: {}",
                        index.to_string_lossy()
                    );
                }
                let mut device = std::ptr::null_mut();
                let rc = ffmpeg::ffi::av_hwdevice_ctx_create(
                    &mut device,
                    kind,
                    adapter.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                    std::ptr::null_mut(),
                    0,
                );
                if rc < 0 {
                    ffmpeg::ffi::av_buffer_unref(&mut device);
                    return Err(ffmpeg::Error::from(rc).into());
                }
                (*context.as_mut_ptr()).hw_device_ctx = device;
                // CUVID always emits CUDA frames. Generic D3D11VA requires an
                // explicit pixel format choice, exactly like the C++ baseline.
                if backend == Backend::D3d11 {
                    (*context.as_mut_ptr()).get_format = Some(hardware_format);
                }
            }
        }
        let inner = context.decoder().open_as(codec)?.video()?;
        Ok(Self {
            inner,
            length_size: config.length_size,
            hardware: backend != Backend::Cpu,
            configuration: config,
            backend,
            requested_hardware: backend != Backend::Cpu,
            pending_configuration: true,
            scratch: Vec::new(),
        })
    }
    pub fn reconfigure(&mut self, config: Configuration, hardware: bool) -> Result<()> {
        if config == self.configuration && hardware == self.requested_hardware {
            return Ok(());
        }
        let mut reopen =
            config.hevc != self.configuration.hevc || hardware != self.requested_hardware;
        #[cfg(windows)]
        {
            reopen |= self.backend == Backend::Nvdec;
        }
        #[cfg(not(windows))]
        {
            let _ = &mut reopen;
        }
        if reopen {
            *self = Self::new(config, hardware)?;
        } else {
            // CPU/D3D11 decode supports SPS changes in-stream. Keep its codec
            // context and feed fresh parameter sets with the next packet.
            self.length_size = config.length_size;
            self.configuration = config;
            self.pending_configuration = true;
        }
        Ok(())
    }
    fn fallback(&self) -> Result<Self> {
        #[cfg(windows)]
        if self.backend == Backend::Nvdec
            && let Ok(mut decoder) = Self::open(self.configuration.clone(), Backend::D3d11)
        {
            decoder.requested_hardware = self.requested_hardware;
            return Ok(decoder);
        }
        let mut decoder = Self::open(self.configuration.clone(), Backend::Cpu)?;
        decoder.requested_hardware = self.requested_hardware;
        Ok(decoder)
    }
    pub fn decode(&mut self, bytes: &mut Vec<u8>) -> Result<Vec<frame::Video>> {
        self.decode_mirror(bytes, false)
    }
    pub fn decode_mirror(&mut self, bytes: &mut Vec<u8>, idr: bool) -> Result<Vec<frame::Video>> {
        self.decode_mirror_at(bytes, idr, None)
    }
    pub fn flush(&mut self) {
        self.inner.flush();
        self.pending_configuration = true;
    }
    pub fn decode_mirror_at(
        &mut self,
        bytes: &mut Vec<u8>,
        idr: bool,
        pts: Option<i64>,
    ) -> Result<Vec<frame::Video>> {
        annex_b(bytes, self.length_size)?;
        let mut packet = if idr || self.pending_configuration {
            self.scratch.clear();
            self.scratch.extend_from_slice(&self.configuration.bytes);
            self.scratch.extend_from_slice(bytes);
            ffmpeg::Packet::copy(&self.scratch)
        } else {
            ffmpeg::Packet::copy(bytes)
        };
        packet.set_pts(pts);
        self.pending_configuration = false;
        match self.packet(&packet) {
            Ok(frames) => Ok(frames),
            Err(e) if self.hardware => {
                tracing::warn!("Hardware decode failed: {e:#}; trying the next decoder backend");
                *self = self.fallback()?;
                match self.packet(&packet) {
                    Err(e) if self.hardware => {
                        tracing::warn!("Hardware fallback failed: {e:#}; using CPU");
                        let requested = self.requested_hardware;
                        *self = Self::open(self.configuration.clone(), Backend::Cpu)?;
                        self.requested_hardware = requested;
                        self.packet(&packet)
                    }
                    result => result,
                }
            }
            Err(e) => Err(e),
        }
    }
    fn packet(&mut self, packet: &ffmpeg::Packet) -> Result<Vec<frame::Video>> {
        self.inner.send_packet(packet)?;
        let mut output = Vec::new();
        loop {
            let mut frame = frame::Video::empty();
            match self.inner.receive_frame(&mut frame) {
                Ok(()) => {}
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
                Err(ffmpeg::Error::Eof) => break,
                Err(e) => return Err(e.into()),
            }
            if matches!(
                frame.format(),
                ffmpeg::format::Pixel::D3D11 | ffmpeg::format::Pixel::CUDA
            ) {
                let mut software = frame::Video::empty();
                unsafe {
                    let rc = ffmpeg::ffi::av_hwframe_transfer_data(
                        software.as_mut_ptr(),
                        frame.as_ptr(),
                        0,
                    );
                    if rc < 0 {
                        bail!(
                            "Hardware frame transfer failed: {}",
                            ffmpeg::Error::from(rc)
                        )
                    }
                    let rc =
                        ffmpeg::ffi::av_frame_copy_props(software.as_mut_ptr(), frame.as_ptr());
                    ensure!(rc >= 0, "Hardware frame property copy failed");
                }
                frame = software;
            }
            output.push(frame);
        }
        Ok(output)
    }
}
#[cfg(windows)]
unsafe extern "C" fn hardware_format(
    _ctx: *mut ffmpeg::ffi::AVCodecContext,
    formats: *const ffmpeg::ffi::AVPixelFormat,
) -> ffmpeg::ffi::AVPixelFormat {
    unsafe {
        let mut p = formats;
        while *p != ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if *p == ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_D3D11 {
                return *p;
            }
            p = p.add(1);
        }
        *formats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_byte_lengths_rewrite_in_place() {
        let mut v = vec![0, 0, 0, 2, 0x65, 0x22, 0, 0, 0, 1, 0x41];
        let original = v.as_ptr();
        annex_b(&mut v, 4).unwrap();
        assert_eq!(v.as_ptr(), original);
        assert_eq!(v, [0, 0, 0, 1, 0x65, 0x22, 0, 0, 0, 1, 0x41]);
    }
    #[test]
    fn reject_truncated_units() {
        assert!(annex_b(&mut vec![0, 0, 0, 20, 0x65], 4).is_err());
    }
    #[test]
    fn length_256_is_a_length_prefix_not_an_annex_b_start_code() {
        let mut bytes = vec![0, 0, 1, 0];
        bytes.extend(std::iter::repeat_n(0x41, 256));
        annex_b(&mut bytes, 4).unwrap();
        assert_eq!(&bytes[..4], &[0, 0, 0, 1]);
        assert_eq!(bytes.len(), 260);
    }
}
