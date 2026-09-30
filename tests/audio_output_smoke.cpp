#include "audio/audio_output.h"

#include <array>
#include <cstdint>

int main() {
    ap::audio::SdlAudioOutput output;
    if (!output.start(44100, 2)) return 1;

    // Ten AAC-ELD-sized stereo PCM blocks exceed the 80 ms start threshold.
    // start() also verifies that SDL kept its logical queue at 44.1 kHz rather
    // than exposing a native 48 kHz device rate to this class.
    std::array<int16_t, 960> silence{};
    for (int i = 0; i < 10; ++i) {
        output.push(silence.data(), static_cast<int>(silence.size()));
    }
    return output.queued_bytes() == 0 ? 2 : 0;
}
