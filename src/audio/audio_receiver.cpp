#include "audio/audio_receiver.h"
#include "log.h"
#include "video/video_renderer.h"

#include <array>
#include <chrono>
#include <cstring>

#if !defined(_WIN32)
    #include <netdb.h>
#endif

#include <openssl/evp.h>

#if defined(_WIN32)
    #include <winsock2.h>
#else
    #include <sys/socket.h>
    #include <sys/time.h>
    #include <errno.h>
#endif

namespace ap::audio {
namespace {

constexpr int kRecvTimeoutMs = 200;

void set_recv_timeout(socket_t s, int ms) {
#if defined(_WIN32)
    DWORD timeout = static_cast<DWORD>(ms);
    ::setsockopt(s, SOL_SOCKET, SO_RCVTIMEO,
                 reinterpret_cast<const char*>(&timeout), sizeof(timeout));
#else
    timeval tv;
    tv.tv_sec  = ms / 1000;
    tv.tv_usec = (ms % 1000) * 1000;
    ::setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
#endif
}

} // namespace

// RAOP compression-type values, from UxPlay/global.h and observed sessions.
const char* ct_name(int ct) {
    switch (ct) {
        case 0: return "unspecified";
        case 1: return "PCM";
        case 2: return "ALAC";
        case 3: return "AAC-LC";
        case 4: return "AAC-ELD";      // common on AirPlay 2 (Apple Music)
        case 8: return "AAC-ELD 44.1k";
        default: return "unknown";
    }
}

AudioReceiver::AudioReceiver()  = default;
AudioReceiver::~AudioReceiver() { stop(); }

bool AudioReceiver::start(Config cfg) {
    cfg_ = std::move(cfg);
    if (cfg_.data_sock == INVALID_SOCK) return false;
    if (cfg_.aes_key.size() != 16 || cfg_.aes_iv.size() != 16) {
        LOG_ERROR << "AudioReceiver: aes_key/aes_iv must be 16 B "
                  << "(got " << cfg_.aes_key.size() << '/' << cfg_.aes_iv.size() << ')';
        return false;
    }

    aes_ctx_ = EVP_CIPHER_CTX_new();
    if (!aes_ctx_ ||
        EVP_DecryptInit_ex(aes_ctx_, EVP_aes_128_cbc(), nullptr,
                           cfg_.aes_key.data(), cfg_.aes_iv.data()) != 1) {
        LOG_ERROR << "AudioReceiver: EVP_DecryptInit_ex(aes-128-cbc) failed";
        if (aes_ctx_) { EVP_CIPHER_CTX_free(aes_ctx_); aes_ctx_ = nullptr; }
        return false;
    }
    EVP_CIPHER_CTX_set_padding(aes_ctx_, 0);

    // AAC-ELD decoder — ct==4 or ct==8 per observed iOS streams. We try
    // to init unconditionally; if the codec is something else (PCM, ALAC)
    // the packets will fail to decode and we'll log but not crash.
    decoder_ = std::make_unique<AacDecoder>();
    AacDecoder::Config dc;
    dc.ct          = cfg_.ct;
    dc.sample_rate = cfg_.sample_rate;
    dc.channels    = 2;
    // AirPlay advertises the AAC-ELD frame length in SETUP. Feeding FFmpeg
    // an ASC for 480 samples when the sender actually uses 512 makes frames
    // intermittently fail to decode. Older clients may omit it, in which
    // case 480 is the commonly observed default.
    dc.spf         = cfg_.spf == 512 ? 512 : 480;
    if (!decoder_->init(dc)) {
        LOG_WARN << "AudioReceiver: AAC decoder init failed — running in "
                    "decrypt-only mode";
        decoder_.reset();
    }

    // SDL-backed audio sink. Goes live only if the OS gave us a device;
    // a headless Linux VM will fall back to silent decode-only mode.
    if (decoder_) {
        output_ = std::make_unique<SdlAudioOutput>();
        if (!output_->start(cfg_.sample_rate, 2)) {
            LOG_WARN << "AudioReceiver: SDL audio output unavailable — "
                        "PCM will be decoded but not played";
            output_.reset();
        }
    }

    set_recv_timeout(cfg_.data_sock, kRecvTimeoutMs);

    running_ = true;
    thread_  = std::thread(&AudioReceiver::thread_fn, this);
    LOG_INFO << "AudioReceiver listening (ct=" << cfg_.ct
             << ' ' << ct_name(cfg_.ct)
             << ", sample_rate=" << cfg_.sample_rate << ')';
    return true;
}

void AudioReceiver::set_volume_db(float db) {
    if (output_) output_->set_volume_db(db);
}

void AudioReceiver::stop() {
    running_.store(false);

    if (cfg_.data_sock != INVALID_SOCK) {
        ap::net::close_socket(cfg_.data_sock);
        cfg_.data_sock = INVALID_SOCK;
    }
    if (cfg_.control_sock != INVALID_SOCK) {
        ap::net::close_socket(cfg_.control_sock);
        cfg_.control_sock = INVALID_SOCK;
    }
    if (thread_.joinable()) thread_.join();

    if (output_)  { output_->stop();  output_.reset();  }
    if (decoder_) {                    decoder_.reset(); }

    if (aes_ctx_) {
        EVP_CIPHER_CTX_free(aes_ctx_);
        aes_ctx_ = nullptr;
    }
}

void AudioReceiver::thread_fn() {
    struct BufferedPacket {
        bool filled = false;
        uint16_t seq = 0;
        uint32_t timestamp = 0;
        std::vector<unsigned char> payload;
    };

    constexpr std::size_t kReorderSlots = 256;
    constexpr auto kResendRetry = std::chrono::milliseconds(10);
    constexpr auto kGapDeadline = std::chrono::milliseconds(30);
    constexpr int kSilencePauseMs = 500;
    constexpr unsigned char kNoDataMarker[4] = {0x00, 0x68, 0x34, 0x00};

    std::array<BufferedPacket, kReorderSlots> reorder{};
    bool have_expected = false;
    uint16_t expected_seq = 0;
    uint16_t highest_seq = 0;
    uint16_t control_seq = 0;
    uint64_t pkts = 0, dedup_dropped = 0, resend_requests = 0;
    uint64_t missing_skipped = 0, total_bytes = 0;
    bool gap_active = false;
    uint16_t gap_seq = 0;
    std::chrono::steady_clock::time_point gap_started{};
    std::chrono::steady_clock::time_point last_resend{};
    auto last_packet = std::chrono::steady_clock::now();
    bool audio_ever_seen = false;

    sockaddr_storage remote_control{};
    socklen_t remote_control_len = 0;
    if (cfg_.control_sock != INVALID_SOCK && cfg_.remote_control_port != 0 &&
        !cfg_.remote_ip.empty()) {
        sockaddr_storage local{};
#if defined(_WIN32)
        int local_len = sizeof(local);
#else
        socklen_t local_len = sizeof(local);
#endif
        ::getsockname(cfg_.control_sock, reinterpret_cast<sockaddr*>(&local),
                      &local_len);
        addrinfo hints{};
        hints.ai_family = local.ss_family;
        hints.ai_socktype = SOCK_DGRAM;
        hints.ai_flags = AI_NUMERICSERV;
#if defined(AI_V4MAPPED)
        if (hints.ai_family == AF_INET6) hints.ai_flags |= AI_V4MAPPED;
#endif
        addrinfo* result = nullptr;
        const std::string port = std::to_string(cfg_.remote_control_port);
        if (::getaddrinfo(cfg_.remote_ip.c_str(), port.c_str(), &hints,
                          &result) == 0 && result) {
            remote_control_len = static_cast<socklen_t>(result->ai_addrlen);
            std::memcpy(&remote_control, result->ai_addr, result->ai_addrlen);
        } else {
            LOG_WARN << "AudioReceiver: cannot resolve remote control endpoint "
                     << cfg_.remote_ip << ':' << cfg_.remote_control_port;
        }
        if (result) ::freeaddrinfo(result);
    }

    auto request_resend = [&](uint16_t first, uint16_t count) {
        if (!count || !remote_control_len) return;
        unsigned char request[8] = {
            0x80, 0xd5,
            static_cast<unsigned char>(control_seq >> 8),
            static_cast<unsigned char>(control_seq),
            static_cast<unsigned char>(first >> 8),
            static_cast<unsigned char>(first),
            static_cast<unsigned char>(count >> 8),
            static_cast<unsigned char>(count)};
        ++control_seq;
        if (::sendto(cfg_.control_sock, reinterpret_cast<const char*>(request),
                     sizeof(request), 0,
                     reinterpret_cast<const sockaddr*>(&remote_control),
                     remote_control_len) >= 0) {
            ++resend_requests;
        }
    };

    auto decode_packet = [&](BufferedPacket& packet) {
        if (!decoder_) return;
        const int got = decoder_->decode(packet.payload.data(),
                                         static_cast<int>(packet.payload.size()));
        if (got <= 0) return;
        int16_t pcm[8192];
        int have = 0;
        while ((have = decoder_->pull_pcm_s16(
                    pcm, static_cast<int>(sizeof(pcm) / sizeof(pcm[0])))) > 0) {
            if (cfg_.renderer)
                cfg_.renderer->push_audio_pcm(pcm, have, cfg_.sample_rate, 2);
            if (output_) output_->push(pcm, have);
        }
    };

    auto conceal_missing_frame = [&] {
        const int frames = cfg_.spf > 0 ? cfg_.spf : (cfg_.ct == 2 ? 352 : 480);
        std::vector<int16_t> silence(static_cast<std::size_t>(frames) * 2, 0);
        if (cfg_.renderer)
            cfg_.renderer->push_audio_pcm(silence.data(),
                                          static_cast<int>(silence.size()),
                                          cfg_.sample_rate, 2);
        if (output_) output_->push(silence.data(), static_cast<int>(silence.size()));
    };

    auto drain = [&] {
        while (have_expected) {
            BufferedPacket& entry = reorder[expected_seq % kReorderSlots];
            if (entry.filled && entry.seq == expected_seq) {
                decode_packet(entry);
                entry.payload.clear();
                entry.filled = false;
                ++expected_seq;
                gap_active = false;
                continue;
            }
            const uint16_t distance = static_cast<uint16_t>(highest_seq - expected_seq);
            if (distance == 0 || distance >= kReorderSlots) {
                gap_active = false;
                break;
            }

            const auto now = std::chrono::steady_clock::now();
            if (!gap_active || gap_seq != expected_seq) {
                gap_active = true;
                gap_seq = expected_seq;
                gap_started = now;
                last_resend = now - kResendRetry;
            }
            if (now - last_resend >= kResendRetry) {
                uint16_t count = 0;
                while (count < distance) {
                    const uint16_t candidate_seq =
                        static_cast<uint16_t>(expected_seq + count);
                    const auto& candidate = reorder[candidate_seq % kReorderSlots];
                    if (candidate.filled && candidate.seq == candidate_seq) break;
                    ++count;
                }
                request_resend(expected_seq, count);
                last_resend = now;
            }
            if (now - gap_started < kGapDeadline) break;

            LOG_WARN << "audio packet " << expected_seq
                     << " was not recovered within " << kGapDeadline.count()
                     << " ms; inserting one silent frame";
            conceal_missing_frame();
            ++missing_skipped;
            ++expected_seq;
            gap_active = false;
        }
    };

    auto enqueue = [&](const unsigned char* packet, int n) {
        if (n < 12 || (packet[1] & 0x7f) != 96) return;
        const uint16_t seq = (static_cast<uint16_t>(packet[2]) << 8) | packet[3];
        const uint32_t ts = (static_cast<uint32_t>(packet[4]) << 24) |
                            (static_cast<uint32_t>(packet[5]) << 16) |
                            (static_cast<uint32_t>(packet[6]) << 8) | packet[7];
        const int payload_len = n - 12;
        ++pkts;
        total_bytes += static_cast<uint64_t>(payload_len);

        if (payload_len == 0 ||
            (payload_len == 4 && std::memcmp(packet + 12, kNoDataMarker, 4) == 0) ||
            (cfg_.ct == 2 && payload_len == 32)) return;

        if (n >= 100) {
            last_packet = std::chrono::steady_clock::now();
            audio_ever_seen = true;
            if (cfg_.renderer && !cfg_.renderer->in_flush_grace())
                cfg_.renderer->push_playback_rate(1.0f);
        }

        if (!have_expected) {
            expected_seq = highest_seq = seq;
            have_expected = true;
        } else {
            const int16_t relative = static_cast<int16_t>(seq - expected_seq);
            if (relative < 0) { ++dedup_dropped; return; }
            if (relative >= static_cast<int>(kReorderSlots)) {
                LOG_WARN << "audio sequence jumped beyond reorder window; resynchronizing";
                for (auto& old : reorder) { old.filled = false; old.payload.clear(); }
                expected_seq = highest_seq = seq;
            } else if (static_cast<int16_t>(seq - highest_seq) > 0) {
                highest_seq = seq;
            }
        }

        BufferedPacket& entry = reorder[seq % kReorderSlots];
        if (entry.filled && entry.seq == seq) { ++dedup_dropped; return; }

        const int encrypted_len = (payload_len / 16) * 16;
        entry.payload.resize(static_cast<std::size_t>(payload_len));
        EVP_DecryptInit_ex(aes_ctx_, nullptr, nullptr, nullptr, cfg_.aes_iv.data());
        int outlen = 0;
        if (encrypted_len)
            EVP_DecryptUpdate(aes_ctx_, entry.payload.data(), &outlen,
                              packet + 12, encrypted_len);
        if (payload_len > encrypted_len)
            std::memcpy(entry.payload.data() + outlen, packet + 12 + encrypted_len,
                        static_cast<std::size_t>(payload_len - encrypted_len));
        entry.seq = seq;
        entry.timestamp = ts;
        entry.filled = true;
        drain();
    };

    unsigned char buf[4096];
    while (running_.load()) {
        fd_set readfds;
        FD_ZERO(&readfds);
        FD_SET(cfg_.data_sock, &readfds);
        socket_t maxfd = cfg_.data_sock;
        if (cfg_.control_sock != INVALID_SOCK) {
            FD_SET(cfg_.control_sock, &readfds);
            if (cfg_.control_sock > maxfd) maxfd = cfg_.control_sock;
        }
        timeval timeout{0, 5000};
        const int ready = ::select(static_cast<int>(maxfd + 1), &readfds,
                                   nullptr, nullptr, &timeout);
        if (ready > 0 && FD_ISSET(cfg_.data_sock, &readfds)) {
            const int n = ::recvfrom(cfg_.data_sock, reinterpret_cast<char*>(buf),
                                     sizeof(buf), 0, nullptr, nullptr);
            if (n > 0) enqueue(buf, n);
        }
        if (ready > 0 && cfg_.control_sock != INVALID_SOCK &&
            FD_ISSET(cfg_.control_sock, &readfds)) {
            const int n = ::recvfrom(cfg_.control_sock, reinterpret_cast<char*>(buf),
                                     sizeof(buf), 0, nullptr, nullptr);
            if (n >= 16 && (buf[1] & 0x7f) == 0x56) enqueue(buf + 4, n - 4);
        }
        // A missing packet may never cause another socket event. Drive resend
        // retries and the bounded concealment deadline from the poll timeout.
        drain();
        if (cfg_.renderer && audio_ever_seen &&
            std::chrono::duration_cast<std::chrono::milliseconds>(
                std::chrono::steady_clock::now() - last_packet).count() >=
                kSilencePauseMs) {
            cfg_.renderer->push_playback_rate(0.0f);
        }
    }

    LOG_INFO << "AudioReceiver stopped (" << pkts << " packets, "
             << total_bytes << " payload bytes, " << dedup_dropped
             << " duplicates, " << resend_requests << " resend requests, "
             << missing_skipped << " missing frames skipped, "
             << (decoder_ ? decoder_->frames_decoded() : 0)
             << " PCM frames decoded)";
}

} // namespace ap::audio
