use std::fs;

use tellurium_assets::{Assets, ScriptAsset};
use tellurium_scripting::{EditorPlugin, LuauConfig};

use crate::Runtime;

#[test]
fn reload_keeps_entity_globals_and_hook_state_and_rolls_back_errors() {
    let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let path = directory.path().join("main.luau");
    fs::write(
        &path,
        r#"
        state = { value = 1 }
        state.self = state
        local counter = 2
        function on_snapshot() return counter end
        function on_start() state.value += 10 end
        function on_update()
            state.value += 1
            counter += 1
            transform.set_position(state.value, counter, 0)
        end
    "#,
    )
    .unwrap();
    let assets = Assets::new(directory.path()).unwrap();
    let load = || {
        assets
            .request::<ScriptAsset>("main.luau")
            .unwrap()
            .wait()
            .unwrap()
    };
    let mut runtime = Runtime::new(LuauConfig::default()).unwrap();
    let entity = {
        let mut world = runtime.world.borrow_mut();
        let entity = world.spawn();
        world.add_component(entity, tellurium_core::Transform::default());
        entity
    };
    let initial = load();
    runtime
        .scripting
        .attach_handle(entity, "main.luau", &initial)
        .unwrap();
    let component = *runtime
        .world
        .borrow()
        .get_component::<tellurium_scripting::ScriptComponent>(entity)
        .unwrap();
    let frame = |runtime: &mut Runtime| {
        runtime.frame(Vec::new(), 1.0 / 60.0, 0.0, |_| ()).unwrap();
    };
    frame(&mut runtime);
    assert_eq!(super::position(&runtime, entity), [12.0, 3.0, 0.0]);
    assert_eq!(runtime.metrics().entities, 1);
    assert!(runtime.metrics().luau_bytes > 0);
    assert!(runtime.metrics().frame_ms > 0.0);
    assert_eq!(runtime.scripting.reload_asset("main.luau", &initial), 0);

    fs::write(
        &path,
        r#"
        state = { value = 0 }
        debug_field = "v2"
        local counter = 0
        transform.set_position(-1, -1, -1)
        function on_start() error("must not start twice") end
        function on_restore(value) counter = value end
        function on_snapshot() return counter end
        function on_update()
            assert(state.self == state)
            state.value += 5
            counter += 2
            transform.set_position(state.value, counter, 1)
        end
    "#,
    )
    .unwrap();
    assert_eq!(runtime.scripting.reload_asset("main.luau", &load()), 1);
    assert_eq!(super::position(&runtime, entity), [12.0, 3.0, 0.0]);
    assert_eq!(
        runtime
            .world
            .borrow()
            .get_component::<tellurium_scripting::ScriptComponent>(entity),
        Some(&component)
    );
    assert!(runtime
        .scripting
        .inspect_globals(entity)
        .iter()
        .any(|(name, value)| name == "debug_field" && value == "v2"));
    frame(&mut runtime);
    assert_eq!(super::position(&runtime, entity), [17.0, 5.0, 1.0]);

    fs::write(
        &path,
        "transform.set_position(99, 99, 99); error('failed reload')",
    )
    .unwrap();
    assert_eq!(runtime.scripting.reload_asset("main.luau", &load()), 0);
    frame(&mut runtime);
    assert_eq!(super::position(&runtime, entity), [22.0, 7.0, 1.0]);
}

#[test]
fn editor_plugins_are_isolated_quota_limited_and_recoverable() {
    let first = EditorPlugin::load(
        "first",
        r#"
        assert(os == nil and io == nil and debug == nil and require == nil)
        assert(entity == nil and transform == nil and getfenv == nil and coroutine == nil)
        local count = 0
        return function(event, path)
            if event == "Loop" then while true do end end
            count += 1
            return event .. ":" .. path .. ":" .. count
        end
    "#,
    )
    .unwrap();
    let second = EditorPlugin::load("second", "return function() return 'ready' end").unwrap();
    assert_eq!(
        first
            .dispatch("SceneOpened", "scenes/main.telsc")
            .unwrap()
            .as_deref(),
        Some("SceneOpened:scenes/main.telsc:1")
    );
    assert!(first.dispatch("Loop", "").is_err());
    assert_eq!(
        second.dispatch("SceneOpened", "").unwrap().as_deref(),
        Some("ready")
    );
    assert_eq!(
        first
            .dispatch("SceneOpened", "scenes/main.telsc")
            .unwrap()
            .as_deref(),
        Some("SceneOpened:scenes/main.telsc:2")
    );
}
