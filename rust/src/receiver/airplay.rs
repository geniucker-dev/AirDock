// SPDX-License-Identifier: MPL-2.0
//! AirPlay wire behavior stays behind the common receiver lifecycle boundary.
pub mod transport;

use super::{ProtocolId, ReceiverBackend};
use crate::{
    config::Settings,
    crypto,
    discovery::{Discovery, advertisement_changed},
    playback::ClockRelation,
    server::{Device, Server},
    state::Shared,
};
use anyhow::Result;
use std::{path::Path, sync::Arc};

/// Mirror timestamps can use Unix seconds while RAOP uses the NTP epoch.
/// Only a plausible live difference may be exposed as an AV estimate.
pub const MIRROR_CLOCK: ClockRelation = ClockRelation::EpochAligned {
    epoch_offset_us: 2_208_988_800_000_000,
    max_skew_us: 10_000_000,
};

/// Translate negotiated AirPlay codec IDs before entering the common decoder.
pub fn audio_configuration(
    ct: u64,
    rate: u32,
    channels: u8,
    spf: u32,
) -> Result<crate::media::audio::Configuration> {
    use crate::media::audio::{alac_config, eld_config};
    anyhow::ensure!(
        matches!(channels, 1 | 2),
        "Only mono and stereo audio are supported"
    );
    let codec = match ct {
        2 => ffmpeg_next::codec::Id::ALAC,
        3 | 4 | 8 => ffmpeg_next::codec::Id::AAC,
        _ => anyhow::bail!("Unsupported AirPlay audio codec {ct}"),
    };
    let extradata = if ct == 2 {
        alac_config(rate, channels, if spf == 0 { 352 } else { spf })
    } else if ct == 3 {
        let i = [
            96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
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
    Ok(crate::media::audio::Configuration {
        codec,
        extradata,
        rate,
        channels,
    })
}

pub struct AirPlay {
    server: Option<Server>,
    discovery: Option<Discovery>,
}
impl AirPlay {
    pub fn bind(directory: &Path, port: u16, shared: Arc<Shared>) -> Result<Self> {
        let identity = crypto::load_identity(&directory.join("identity.key"))?;
        Ok(Self {
            server: Some(Server::start(Device::new(identity, port, shared)?)?),
            discovery: None,
        })
    }
}
impl ReceiverBackend for AirPlay {
    fn protocol(&self) -> ProtocolId {
        ProtocolId::AIRPLAY
    }
    fn refresh(&mut self, before: &Settings, after: &Settings) -> Result<()> {
        let Some(server) = &self.server else {
            return Ok(());
        };
        if self.discovery.is_none()
            || self
                .discovery
                .as_ref()
                .is_some_and(Discovery::registration_failed)
            || advertisement_changed(before, after)
        {
            // Await withdrawal before publishing replacement records.
            drop(self.discovery.take());
            self.discovery = Some(Discovery::start(server.device())?);
        }
        Ok(())
    }
    fn disconnect(&mut self) {
        // Common cancellation is observed by every AirPlay control connection.
    }
    fn timed_video_playing(&self) -> bool {
        self.server
            .as_ref()
            .is_some_and(|s| s.device().hls.is_playing())
    }
    fn shutdown(&mut self) {
        drop(self.discovery.take());
        drop(self.server.take());
    }
}
impl Drop for AirPlay {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ReconnectPolicy;
    use std::{
        io::{Read, Write},
        net::TcpStream,
        sync::atomic::Ordering,
        time::Duration,
    };

    #[test]
    fn airplay_controls_and_shutdown_cannot_interrupt_another_protocol() {
        let directory = std::env::temp_dir().join(format!(
            "airdock-adapter-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let shared = Shared::new(Settings::default());
        let mut receiver = AirPlay::bind(&directory, 0, shared.clone()).unwrap();
        let port = receiver.server.as_ref().unwrap().device().port;
        let lease = shared
            .sessions
            .claim(
                shared.sessions.allocate_id(),
                ProtocolId("test-cast"),
                "127.0.0.1".parse().unwrap(),
                ReconnectPolicy::Reject,
                Duration::ZERO,
            )
            .unwrap();
        shared.sessions.begin_timed_video(lease.id);
        let mut connection = TcpStream::connect(("127.0.0.1", port)).unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = std::io::BufReader::new(connection.try_clone().unwrap());
        for path in ["/rate?value=0", "/stop", "/audioMode"] {
            connection
                .write_all(
                    format!("POST {path} HTTP/1.1\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n")
                        .as_bytes(),
                )
                .unwrap();
            let mut status = String::new();
            std::io::BufRead::read_line(&mut reader, &mut status).unwrap();
            assert!(status.contains("503"), "{status}");
            loop {
                let mut line = String::new();
                std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                assert!(!line.is_empty());
            }
        }
        assert!(!shared.media.audio_clock.paused.load(Ordering::Acquire));
        let generation = shared.generation();
        receiver.shutdown(); // Active TCP readers must exit without global cancellation.
        receiver.shutdown();
        drop(receiver);
        assert!(shared.running.load(Ordering::Acquire));
        assert!(!lease.stop.load(Ordering::Acquire));
        assert_eq!(shared.sessions.owner_id(), Some(lease.id));
        assert_eq!(shared.generation(), generation);
        assert_eq!(connection.read(&mut [0; 1]).unwrap(), 0);
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
        shared.sessions.release(&lease, || {});
        shared.media.stop();
        std::fs::remove_dir_all(directory).unwrap();
    }
}
