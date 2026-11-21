use std::fs;

use tellurium_core::EntityId;
use tellurium_editor::samples::CATALOG;
use tellurium_platform::{KeyCode, RawInputEvent};
use tellurium_serialization::{ComponentRegistry, PackedScene};

use crate::{scene::PreparedScene, Runtime, FIXED_DELTA};

fn node(scene: &PackedScene, name: &str) -> EntityId {
    scene
        .nodes
        .iter()
        .find(|node| node.name == name)
        .unwrap()
        .id
}

fn advance(runtime: &mut Runtime, frames: usize, key: Option<KeyCode>, time: &mut f64) {
    for index in 0..frames {
        let events = if index == 0 {
            key.map(|key| RawInputEvent::KeyPressed { key })
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };
        *time += FIXED_DELTA;
        runtime.frame(events, FIXED_DELTA, *time, |_| ()).unwrap();
        runtime.finish_physics().unwrap();
    }
}

#[test]
fn built_in_samples_play_export_and_keep_local_edits() -> Result<(), Box<dyn std::error::Error>> {
    let sandbox = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let output = sandbox.path().join("export");
    fs::create_dir(&output)?;
    for sample in CATALOG {
        let project = sample.open(sandbox.path())?;
        let scene = PackedScene::load(&project.path(&project.config.main_scene)?)?;
        let mut game = PreparedScene::load(&project, scene.clone())?
            .install_headless(&project, &ComponentRegistry::default())?;
        let mut elapsed = 0.0;
        match sample.name {
            "Neon Courier" => {
                let courier = node(&scene, "Courier");
                advance(&mut game, 37, Some(KeyCode::W), &mut elapsed);
                assert!(super::position(&game, courier)[1] > 1.2);
                let beacon = node(&scene, "Active checkpoint");
                assert_eq!(super::position(&game, beacon)[0], 0.0);
            }
            "Gravity Foundry" => {
                let crate_id = node(&scene, "FBX crate 1");
                advance(&mut game, 95, Some(KeyCode::Space), &mut elapsed);
                assert!(super::position(&game, crate_id)[1] < 0.8);
                let lamp = node(&scene, "Status lamp");
                assert!(super::position(&game, lamp)[1] > 0.9);
                advance(&mut game, 155, None, &mut elapsed);
                let last_domino = node(&scene, "Domino 8");
                assert!(super::position(&game, last_domino)[1] < 0.35);
            }
            "Orbit Soundstage" => {
                let cyan = node(&scene, "Cyan · Ogg emitter");
                let gold = node(&scene, "Gold · WAV beacon");
                advance(&mut game, 40, Some(KeyCode::T), &mut elapsed);
                assert!(super::position(&game, cyan)[2] > 1.5);
                assert!(super::position(&game, gold)[2] < -1.0);
                assert!(game.audio.is_some());
            }
            name => panic!("Unexpected sample {name}"),
        }
        game.shutdown();
        let exe = tellurium_builder::export_windows(&project, &output)?;
        assert!(exe.is_file());
        assert!(exe.with_extension("telpak").is_file());
        let mut edited = project;
        edited.config.description = "Local edit".into();
        edited.save()?;
        assert_eq!(
            sample.open(sandbox.path())?.config.description,
            "Local edit"
        );
    }
    assert_eq!(CATALOG.len(), 3);
    Ok(())
}
