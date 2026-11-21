use std::fs;

use serde::{Deserialize, Serialize};
use tellurium_core::{Component, EntityId};
use tellurium_serialization::{
    ComponentRegistry, DocumentFormat, Node, NodeKind, PackedScene, Project, ProjectConfig,
    ScriptData, WorldSnapshot,
};

use crate::scene::PreparedScene;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Target(EntityId);

impl Component for Target {}

#[test]
fn scene_instances_have_independent_ids_and_reject_cycles() {
    let mut prefab = PackedScene::default();
    let root = prefab
        .insert(Node::new("Parent", NodeKind::Node3D), None)
        .unwrap();
    let child = prefab
        .insert(Node::new("Child", NodeKind::MeshInstance), Some(root))
        .unwrap();
    assert!(prefab.reparent(root, Some(child)).is_err());

    let mut scene = PackedScene::default();
    let a = scene.instance("A", &prefab, None).unwrap();
    let b = scene.instance("B", &prefab, None).unwrap();
    let ids: Vec<_> = scene
        .order()
        .unwrap()
        .into_iter()
        .map(|index| scene.nodes[index].id)
        .collect();
    assert_eq!(ids.len(), 6);
    assert_eq!(scene.node(ids[1]).unwrap().parent, Some(a));
    assert_eq!(scene.node(ids[2]).unwrap().parent, Some(ids[1]));
    assert_eq!(scene.node(ids[4]).unwrap().parent, Some(b));
    scene.remove_subtree(a);
    assert_eq!(scene.nodes.len(), 3);
    assert!(scene.insert(Node::default(), None).unwrap() > *ids.last().unwrap());
}

#[test]
fn luau_world_snapshot_roundtrips_in_both_formats() {
    let dir = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let project = Project::new(dir.path().join("project.toml"), ProjectConfig::default()).unwrap();
    fs::write(
        dir.path().join("motion.luau"),
        r#"
state = { frames = 0, offsets = {2, 4, 8} }
local offset = 0
function on_start()
    state.frames += 10
end
function on_update()
    state.frames += 1
    offset += state.offsets[2]
    transform.set_position(state.frames, offset, 0)
end
function on_snapshot()
    return offset
end
function on_restore(saved_offset)
    offset = saved_offset
end
"#,
    )
    .unwrap();
    let mut registry = ComponentRegistry::default();
    registry.register::<Target>("target").unwrap();
    let mut scene = PackedScene::default();
    let root = scene
        .insert(Node::new("Root", NodeKind::Node3D), None)
        .unwrap();
    let mut child = Node::new("Child", NodeKind::MeshInstance);
    child.transform.position = [3.0, 0.0, 0.0];
    child.custom.insert("target".into(), root.0.to_string());
    let child = scene.insert(child, Some(root)).unwrap();
    scene.node_mut(root).unwrap().script = Some(ScriptData {
        path: "motion.luau".into(),
        state: None,
    });
    scene.last_entity_id = 400;

    for format in [DocumentFormat::Text, DocumentFormat::Binary] {
        let path = dir.path().join("main.telsc");
        scene.save(&path, format).unwrap();
        let loaded = PackedScene::load(&path).unwrap();
        let mut runtime = PreparedScene::load(&project, loaded)
            .unwrap()
            .install_headless(&project, &registry)
            .unwrap();
        runtime
            .frame(Vec::new(), 1.0 / 60.0, 1.0 / 60.0, |_| ())
            .unwrap();
        assert_eq!(super::position(&runtime, root), [11.0, 4.0, 0.0]);
        assert_eq!(super::position(&runtime, child), [14.0, 4.0, 0.0]);

        let path = dir.path().join("world.telsave");
        runtime
            .snapshot(1.0 / 60.0, &registry)
            .unwrap()
            .save(&path, format)
            .unwrap();
        let snapshot = WorldSnapshot::load(&path).unwrap();
        assert_eq!(snapshot.scene.node(child).unwrap().parent, Some(root));
        let mut restored = PreparedScene::load(&project, snapshot.scene.clone())
            .unwrap()
            .install_headless(&project, &registry)
            .unwrap();
        restored.restore_clock(&snapshot);
        assert_eq!(
            restored.world().borrow().get_component(child),
            Some(&Target(root))
        );
        assert_eq!(restored.world().borrow_mut().spawn(), EntityId(401));
        assert_eq!(super::position(&restored, child), [14.0, 4.0, 0.0]);
        restored
            .frame(Vec::new(), 1.0 / 60.0, 2.0 / 60.0, |_| ())
            .unwrap();
        assert_eq!(super::position(&restored, root), [12.0, 8.0, 0.0]);
        assert_eq!(super::position(&restored, child), [15.0, 8.0, 0.0]);
        restored.shutdown();
        runtime.shutdown();
    }
}
