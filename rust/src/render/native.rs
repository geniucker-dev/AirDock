// SPDX-License-Identifier: MPL-2.0
use anyhow::Result;
use ffmpeg_next::ffi;
use std::ffi::CStr;
pub fn native_report() -> Result<serde_json::Value> {
    unsafe {
        let string = |p: *const std::ffi::c_char| CStr::from_ptr(p).to_string_lossy().into_owned();
        let mut opaque = std::ptr::null_mut();
        let mut encoders = Vec::new();
        let mut decoders = Vec::new();
        loop {
            let codec = ffi::av_codec_iterate(&mut opaque);
            if codec.is_null() {
                break;
            }
            let name = string((*codec).name);
            if ffi::av_codec_is_encoder(codec) != 0 {
                encoders.push(name)
            } else {
                decoders.push(name)
            }
        }
        opaque = std::ptr::null_mut();
        let mut demuxers = Vec::new();
        loop {
            let demuxer = ffi::av_demuxer_iterate(&mut opaque);
            if demuxer.is_null() {
                break;
            }
            demuxers.push(string((*demuxer).name));
        }
        let mut state = std::ptr::null_mut();
        let mut protocols = Vec::new();
        loop {
            let protocol = ffi::avio_enum_protocols(&mut state, 0);
            if protocol.is_null() {
                break;
            }
            protocols.push(string(protocol));
        }
        Ok(
            serde_json::json!({"encoders":encoders,"decoders":decoders,"demuxers":demuxers,"input_protocols":protocols,"avcodec":{"version":ffi::avcodec_version(),"configuration":string(ffi::avcodec_configuration()),"license":string(ffi::avcodec_license())},"avformat":{"version":ffi::avformat_version(),"configuration":string(ffi::avformat_configuration()),"license":string(ffi::avformat_license())},"avutil":{"version":ffi::avutil_version(),"configuration":string(ffi::avutil_configuration()),"license":string(ffi::avutil_license())},"swresample":{"version":ffi::swresample_version(),"configuration":string(ffi::swresample_configuration()),"license":string(ffi::swresample_license())}}),
        )
    }
}
