use std::collections::{HashMap, HashSet};

use glam::{Quat, Vec3};
use tellurium_assets::{AssetRevision, AudioClip, Handle};
use tellurium_bridge::{
    create_audio_device, AudioClipId, AudioDevice, AudioVoiceId, ListenerState, NativeError,
    NativePtr, VoiceState,
};
use tellurium_core::{Component, EntityId, Transform, World};

use crate::physics::Rigidbody;

pub(crate) struct AudioSource {
    pub clip: Handle<AudioClip>,
    pub volume: f32,
    pub looping: bool,
    pub spatial: bool,
    pub transient: bool,
}

impl Component for AudioSource {}

pub(crate) struct AudioListener;

impl Component for AudioListener {}

struct ActiveVoice {
    clip: AssetRevision,
    id: AudioVoiceId,
    position: Vec3,
}

pub(crate) struct AudioSystem {
    device: NativePtr<AudioDevice>,
    clips: HashMap<AssetRevision, AudioClipId>,
    voices: HashMap<EntityId, ActiveVoice>,
    listener_position: Option<(EntityId, Vec3)>,
    offline: bool,
    mix_buffer: Vec<f32>,
}

impl AudioSystem {
    pub(crate) fn new() -> Result<Self, NativeError> {
        let (device, offline) = match create_audio_device(false) {
            Ok(device) => (device, false),
            Err(error) => {
                log::warn!("Audio device unavailable ({error}). Running the mixer without an output device.");
                (create_audio_device(true)?, true)
            }
        };
        log::info!("miniaudio output: {}", device.output_name());
        Ok(Self {
            device,
            clips: HashMap::new(),
            voices: HashMap::new(),
            listener_position: None,
            offline,
            mix_buffer: Vec::new(),
        })
    }

    #[cfg(all(feature = "editor", windows))]
    pub(crate) fn set_paused(&mut self, paused: bool) -> Result<(), NativeError> {
        self.device.pin_mut().set_audio_paused(paused)
    }

    pub(crate) fn update(&mut self, world: &mut World, delta: f64) -> Result<(), NativeError> {
        self.listener(world, delta)?;
        self.voices.retain(|entity, voice| {
            let retained = world
                .get_component::<AudioSource>(*entity)
                .is_some_and(|source| source.clip.revision() == voice.clip)
                && world.has_component::<Transform>(*entity);
            if !retained {
                self.device.pin_mut().release_audio_voice(voice.id);
            }
            retained
        });

        let mut expired = Vec::new();
        for (entity, transform, source) in world.query2::<Transform, AudioSource>() {
            let position = Vec3::from_array(transform.position);
            let revision = source.clip.revision();
            let previous = self
                .voices
                .get(&entity)
                .map_or(position, |voice| voice.position);
            let velocity = world.get_component::<Rigidbody>(entity).map_or_else(
                || estimate_velocity(previous, position, delta),
                |body| body.linear_velocity,
            );
            let state = VoiceState {
                position: transform.position,
                velocity,
                volume: source.volume,
                min_distance: 1.0,
                max_distance: 32.0,
                rolloff: 1.0,
                doppler: 1.0,
                looping: source.looping,
                spatial: source.spatial,
            };

            if let Some(voice) = self.voices.get_mut(&entity) {
                if !source.looping && self.device.pin_mut().audio_voice_finished(voice.id)? {
                    expired.push((entity, source.transient));
                    continue;
                }
                self.device.pin_mut().update_audio_voice(voice.id, &state)?;
                voice.position = position;
                continue;
            }

            let handle = match self.clips.get(&revision) {
                Some(&handle) => handle,
                None => {
                    let clip = &source.clip;
                    let handle = self.device.pin_mut().upload_audio(
                        clip.sample_rate,
                        clip.channels,
                        &clip.samples,
                    )?;
                    self.clips.insert(revision, handle);
                    handle
                }
            };
            let id = self.device.pin_mut().play_audio(handle, &state)?;
            self.voices.insert(
                entity,
                ActiveVoice {
                    clip: revision,
                    id,
                    position,
                },
            );
        }

        for (entity, transient) in expired {
            if let Some(voice) = self.voices.remove(&entity) {
                self.device.pin_mut().release_audio_voice(voice.id);
            }
            if transient {
                world.despawn(entity);
            } else {
                world.remove_component::<AudioSource>(entity);
            }
        }

        let live_clips: HashSet<_> = self.voices.values().map(|voice| voice.clip).collect();
        self.clips.retain(|revision, handle| {
            let retained = live_clips.contains(revision);
            if !retained {
                self.device.pin_mut().release_audio_clip(*handle);
            }
            retained
        });

        if self.offline {
            self.mix_buffer
                .resize((delta.clamp(0.0, 0.1) * 48000.0) as usize * 2, 0.0);
            self.device.pin_mut().mix_offline(&mut self.mix_buffer)?;
        }
        Ok(())
    }

    fn listener(&mut self, world: &World, delta: f64) -> Result<(), NativeError> {
        let mut state = ListenerState {
            position: [0.0; 3],
            velocity: [0.0; 3],
            forward: [0.0, 0.0, 1.0],
            up: [0.0, 1.0, 0.0],
        };
        if let Some((entity, pose, _)) = world
            .query2::<Transform, AudioListener>()
            .min_by_key(|(entity, _, _)| *entity)
        {
            let position = Vec3::from_array(pose.position);
            let rotation = Quat::from_array(pose.rotation);
            let rotation = if rotation.is_finite() && rotation.length_squared() > 1.0e-8 {
                rotation.normalize()
            } else {
                Quat::IDENTITY
            };
            state.position = pose.position;
            state.forward = (rotation * Vec3::Z).to_array();
            state.up = (rotation * Vec3::Y).to_array();
            if let Some((owner, previous)) = self.listener_position {
                if owner == entity {
                    state.velocity = estimate_velocity(previous, position, delta);
                }
            }
            self.listener_position = Some((entity, position));
        } else {
            self.listener_position = None;
        }
        self.device.pin_mut().update_audio_listener(&state)
    }
}

fn estimate_velocity(before: Vec3, now: Vec3, delta: f64) -> [f32; 3] {
    if delta > 0.001 {
        return ((now - before) / delta as f32)
            .clamp_length_max(100.0)
            .to_array();
    }
    [0.0; 3]
}
