use tellurium_bridge::{
    create_audio_device, create_physics_world, raycast, step_physics, sync_physics_bodies,
    take_physics_frame, BodyDesc, BodyKind, ColliderKind, ListenerState, Ray, Transform,
    VoiceState,
};

fn body(entity: u64, kind: BodyKind, y: f32, half_extents: [f32; 3], trigger: bool) -> BodyDesc {
    BodyDesc {
        entity,
        pose: Transform {
            position: [0.0, y, 0.0],
            ..Transform::default()
        },
        linear_velocity: [0.0; 3],
        kind,
        collider: ColliderKind::Box,
        dimensions: half_extents,
        mass: if kind == BodyKind::Dynamic { 1.0 } else { 0.0 },
        trigger,
        override_pose: true,
        override_velocity: true,
    }
}

#[test]
fn bullet_fall_trigger_raycast_and_removal_use_ecs_ids() {
    let floor = body(1, BodyKind::Static, -0.25, [5.0, 0.25, 5.0], false);
    let cube = body(2, BodyKind::Dynamic, 3.0, [0.3; 3], false);
    let sensor = body(3, BodyKind::Static, 1.2, [1.0, 0.2, 1.0], true);
    let mut world = create_physics_world(-9.81).unwrap();
    sync_physics_bodies(world.pin_mut(), &[floor, cube, sensor]).unwrap();
    let ray = Ray {
        origin: [0.0, 6.0, 0.0],
        direction: [0.0, -1.0, 0.0],
        max_distance: 10.0,
    };
    assert_eq!(raycast(&world, &ray).unwrap().entity, cube.entity);
    assert!(raycast(
        &world,
        &Ray {
            direction: [0.0; 3],
            ..ray
        }
    )
    .is_err());

    for _ in 0..180 {
        step_physics(world.pin_mut(), 1.0 / 60.0).unwrap();
    }
    let frame = take_physics_frame(world.pin_mut()).unwrap();
    assert_eq!(frame.bodies.len(), 1);
    assert!((frame.bodies[0].position[1] - 0.3).abs() < 0.03);
    assert!(frame
        .contacts
        .iter()
        .any(|event| event.first == 2 && event.second == 3 && event.trigger));
    assert!(frame
        .contacts
        .iter()
        .any(|event| event.first == 1 && event.second == 2 && !event.trigger));
    assert!(take_physics_frame(world.pin_mut())
        .unwrap()
        .contacts
        .is_empty());

    sync_physics_bodies(world.pin_mut(), &[floor]).unwrap();
    let hit = raycast(&world, &ray).unwrap();
    assert!(hit.hit);
    assert_eq!(hit.entity, floor.entity);
    assert!((hit.distance - 6.0).abs() < 0.001);
}

fn mix(position: [f32; 3], velocity: [f32; 3]) -> (f32, f32, usize) {
    let mut device = create_audio_device(true).unwrap();
    device
        .pin_mut()
        .update_audio_listener(&ListenerState {
            position: [0.0; 3],
            velocity: [0.0; 3],
            forward: [0.0, 0.0, 1.0],
            up: [0.0, 1.0, 0.0],
        })
        .unwrap();
    let pcm: Vec<f32> = (0..48000)
        .map(|frame| (frame as f32 * std::f32::consts::TAU * 440.0 / 48000.0).sin() * 0.25)
        .collect();
    let clip = device.pin_mut().upload_audio(48000, 1, &pcm).unwrap();
    let voice = device
        .pin_mut()
        .play_audio(
            clip,
            &VoiceState {
                position,
                velocity,
                volume: 1.0,
                min_distance: 1.0,
                max_distance: 64.0,
                rolloff: 1.0,
                doppler: 1.0,
                looping: true,
                spatial: true,
            },
        )
        .unwrap();
    device.pin_mut().release_audio_clip(clip);
    let mut output = vec![0.0; 48000];
    for period in output.chunks_mut(960) {
        device.pin_mut().mix_offline(period).unwrap();
    }
    let mut channels = [0.0; 2];
    let mut increasing_crossings = 0;
    let mut previous = 0.0;
    for frame in output.as_chunks::<2>().0.iter().skip(1000) {
        channels[0] += frame[0] * frame[0];
        channels[1] += frame[1] * frame[1];
        if previous <= 0.0 && frame[0] > 0.0 {
            increasing_crossings += 1;
        }
        previous = frame[0];
    }
    device.pin_mut().release_audio_voice(voice);
    (channels[0], channels[1], increasing_crossings)
}

#[test]
fn miniaudio_spatial_mix_pans_attenuates_and_applies_doppler() {
    let (near_l, near_r, _) = mix([-1.0, 0.0, 1.0], [0.0; 3]);
    let (far_l, far_r, _) = mix([-8.0, 0.0, 8.0], [0.0; 3]);
    let (_, _, stationary) = mix([0.0, 0.0, 4.0], [0.0; 3]);
    let (_, _, approaching) = mix([0.0, 0.0, 4.0], [0.0, 0.0, -80.0]);
    assert!(
        near_l > 1.0 && near_l > 2.0 * near_r,
        "pan energies: {near_l}, {near_r}"
    );
    assert!(
        near_l + near_r > 8.0 * (far_l + far_r),
        "attenuation energies: {near_l}, {near_r}, {far_l}, {far_r}"
    );
    assert!(
        approaching > stationary + 20,
        "Doppler crossings: {stationary}, {approaching}"
    );
}
