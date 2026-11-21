#include "tellurium-bridge/src/ffi.rs.h"
#include "tellurium/miniaudio_config.h"

#include <algorithm>
#include <cmath>
#include <map>
#include <stdexcept>
#include <string>
#include <vector>

namespace tellurium {
namespace {

void check(ma_result result, const char* operation) {
    if (result != MA_SUCCESS) {
        throw std::runtime_error(std::string(operation) + ": " + ma_result_description(result));
    }
}

struct Clip {
    std::uint32_t sample_rate;
    std::uint32_t channels;
    std::vector<float> samples;
};

struct Voice {
    std::shared_ptr<Clip> clip;
    ma_audio_buffer buffer{};
    ma_sound sound{};

    Voice(ma_engine& engine, std::shared_ptr<Clip> data) : clip(std::move(data)) {
        auto config = ma_audio_buffer_config_init(ma_format_f32, clip->channels,
                                                  clip->samples.size() / clip->channels,
                                                  clip->samples.data(), nullptr);
        config.sampleRate = clip->sample_rate;
        check(ma_audio_buffer_init(&config, &buffer), "ma_audio_buffer_init");
        const auto result = ma_sound_init_from_data_source(&engine, &buffer, 0, nullptr, &sound);
        if (result != MA_SUCCESS) {
            ma_audio_buffer_uninit(&buffer);
            check(result, "ma_sound_init_from_data_source");
        }
    }

    ~Voice() {
        ma_sound_uninit(&sound);
        ma_audio_buffer_uninit(&buffer);
    }

    void update(const VoiceState& state) {
        ma_sound_set_volume(&sound, state.volume);
        ma_sound_set_looping(&sound, state.looping);
        ma_sound_set_spatialization_enabled(&sound, state.spatial);
        ma_sound_set_min_distance(&sound, state.min_distance);
        ma_sound_set_max_distance(&sound, state.max_distance);
        ma_sound_set_rolloff(&sound, state.rolloff);
        ma_sound_set_doppler_factor(&sound, state.doppler);
        ma_sound_set_attenuation_model(&sound, ma_attenuation_model_inverse);
        // The game camera faces +Z. The miniaudio listener faces -Z.
        ma_sound_set_position(&sound, state.position[0], state.position[1], -state.position[2]);
        ma_sound_set_velocity(&sound, state.velocity[0], state.velocity[1], -state.velocity[2]);
    }
};

template <typename T>
T& lookup(std::map<std::uint32_t, T>& entries, std::uint32_t id, const char* kind) {
    const auto entry = entries.find(id);
    if (entry == entries.end()) {
        throw std::invalid_argument(std::string("unknown audio ") + kind);
    }
    return entry->second;
}

template <std::size_t N> bool finite(const std::array<float, N>& values) {
    return std::all_of(values.begin(), values.end(), [](float value) { return std::isfinite(value); });
}

void validate(const VoiceState& state) {
    if (!finite(state.position) || !finite(state.velocity) || !std::isfinite(state.volume) ||
        state.volume < 0.0f || !std::isfinite(state.min_distance) || state.min_distance <= 0.0f ||
        !std::isfinite(state.max_distance) || state.max_distance < state.min_distance ||
        !std::isfinite(state.rolloff) || state.rolloff < 0.0f || !std::isfinite(state.doppler) ||
        state.doppler < 0.0f) {
        throw std::invalid_argument("invalid audio voice settings");
    }
}

std::uint32_t next_id(std::uint32_t& counter) {
    if (counter == UINT32_MAX) {
        throw std::runtime_error("audio handles exhausted");
    }
    return ++counter;
}

} // namespace

struct AudioDevice::Impl {
    ma_engine engine{};
    std::map<std::uint32_t, std::shared_ptr<Clip>> clips;
    std::map<std::uint32_t, std::unique_ptr<Voice>> voices;
    std::uint32_t last_clip = 0;
    std::uint32_t last_voice = 0;
    bool offline;

    explicit Impl(bool offline) : offline(offline) {
        auto config = ma_engine_config_init();
        config.channels = 2;
        config.sampleRate = 48000;
        config.noDevice = offline;
        check(ma_engine_init(&config, &engine), "ma_engine_init");
    }

    ~Impl() {
        voices.clear();
        ma_engine_uninit(&engine);
    }
};

AudioDevice::AudioDevice(bool offline) : impl_(std::make_unique<Impl>(offline)) {}
AudioDevice::~AudioDevice() = default;

rust::String AudioDevice::output_name() const {
    if (impl_->offline) {
        return "offline stereo mixer";
    }
    return ma_engine_get_device(&impl_->engine)->playback.name;
}

AudioClipId AudioDevice::upload_audio(std::uint32_t sample_rate, std::uint32_t channels,
                                      rust::Slice<const float> samples) {
    if (sample_rate == 0 || sample_rate > 384000 || channels == 0 || channels > MA_MAX_CHANNELS ||
        samples.empty() || samples.size() % channels != 0 ||
        !std::all_of(samples.begin(), samples.end(), [](float value) { return std::isfinite(value); })) {
        throw std::invalid_argument("invalid interleaved audio PCM");
    }
    const auto id = next_id(impl_->last_clip);
    auto clip = std::make_shared<Clip>();
    clip->sample_rate = sample_rate;
    clip->channels = channels;
    clip->samples.assign(samples.begin(), samples.end());
    impl_->clips.emplace(id, std::move(clip));
    return {id};
}

void AudioDevice::release_audio_clip(AudioClipId handle) noexcept {
    impl_->clips.erase(handle.id);
}

AudioVoiceId AudioDevice::play_audio(AudioClipId handle, const VoiceState& state) {
    validate(state);
    const auto& clip = lookup(impl_->clips, handle.id, "clip");
    const auto id = next_id(impl_->last_voice);
    auto voice = std::make_unique<Voice>(impl_->engine, clip);
    voice->update(state);
    check(ma_sound_start(&voice->sound), "ma_sound_start");
    impl_->voices.emplace(id, std::move(voice));
    return {id};
}

void AudioDevice::update_audio_voice(AudioVoiceId handle, const VoiceState& state) {
    validate(state);
    lookup(impl_->voices, handle.id, "voice")->update(state);
}

void AudioDevice::release_audio_voice(AudioVoiceId handle) noexcept {
    impl_->voices.erase(handle.id);
}

bool AudioDevice::audio_voice_finished(AudioVoiceId handle) {
    return ma_sound_at_end(&lookup(impl_->voices, handle.id, "voice")->sound) != 0;
}

void AudioDevice::update_audio_listener(const ListenerState& state) {
    if (!finite(state.position) || !finite(state.velocity) || !finite(state.forward) ||
        !finite(state.up)) {
        throw std::invalid_argument("invalid audio listener");
    }
    auto& engine = impl_->engine;
    ma_engine_listener_set_position(&engine, 0, state.position[0], state.position[1],
                                    -state.position[2]);
    ma_engine_listener_set_velocity(&engine, 0, state.velocity[0], state.velocity[1],
                                    -state.velocity[2]);
    ma_engine_listener_set_direction(&engine, 0, state.forward[0], state.forward[1], -state.forward[2]);
    ma_engine_listener_set_world_up(&engine, 0, state.up[0], state.up[1], -state.up[2]);
}

void AudioDevice::set_audio_paused(bool paused) {
    if (!impl_->offline) {
        const auto result = paused ? ma_engine_stop(&impl_->engine) : ma_engine_start(&impl_->engine);
        check(result, paused ? "ma_engine_stop" : "ma_engine_start");
    }
}

void AudioDevice::mix_offline(rust::Slice<float> stereo) {
    if (!impl_->offline || stereo.size() % 2 != 0) {
        throw std::invalid_argument("offline mixing requires an offline device and stereo PCM storage");
    }
    if (!stereo.empty()) {
        check(ma_engine_read_pcm_frames(&impl_->engine, stereo.data(), stereo.size() / 2, nullptr),
              "ma_engine_read_pcm_frames");
    }
}

std::unique_ptr<AudioDevice> create_audio_device(bool offline) {
    return std::make_unique<AudioDevice>(offline);
}

} // namespace tellurium
