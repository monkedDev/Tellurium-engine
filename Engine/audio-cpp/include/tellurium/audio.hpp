#pragma once

#include "rust/cxx.h"

#include <cstdint>
#include <memory>

namespace tellurium {

struct AudioClipId;
struct AudioVoiceId;
struct VoiceState;
struct ListenerState;

class AudioDevice {
  public:
    explicit AudioDevice(bool offline);
    ~AudioDevice();

    rust::String output_name() const;
    AudioClipId upload_audio(std::uint32_t sample_rate, std::uint32_t channels,
                             rust::Slice<const float> samples);
    void release_audio_clip(AudioClipId handle) noexcept;
    AudioVoiceId play_audio(AudioClipId handle, const VoiceState& state);
    void update_audio_voice(AudioVoiceId handle, const VoiceState& state);
    void release_audio_voice(AudioVoiceId handle) noexcept;
    bool audio_voice_finished(AudioVoiceId handle);
    void update_audio_listener(const ListenerState& state);
    void set_audio_paused(bool paused);
    void mix_offline(rust::Slice<float> stereo);

  private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

std::unique_ptr<AudioDevice> create_audio_device(bool offline);

} // namespace tellurium
