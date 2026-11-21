use super::*;
mod input_context;
mod scenes;
use tellurium_core::ContactEvent;
use tellurium_platform::{KeyCode, RawInputState};
use tellurium_scripting::ScriptComponent;

fn runtime() -> Runtime {
    Runtime::new(LuauConfig {
        max_cpu_seconds_per_frame: 1.0,
        ..LuauConfig::default()
    })
    .unwrap()
}

fn position(runtime: &Runtime, entity: EntityId) -> [f32; 3] {
    runtime
        .world
        .borrow()
        .get_component::<Transform>(entity)
        .unwrap()
        .position
}

#[test]
fn input_and_script_state_survive_until_ecs_update() {
    let mut runtime = runtime();
    let first = runtime.spawn("first", DEMO_SOURCE).unwrap();
    let second = runtime
        .spawn(
            "second",
            r#"
        local frames = 0
        function on_update()
            frames += 1
            if frames == 1 then
                assert(input.count() == 3)
                assert(input.event(1).key == "Space")
                assert(input.event(2).kind == "mouse_moved" and input.event(2).x == 32)
                local pad = input.event(3)
                assert(pad.kind == "gamepad_state" and pad.id == 1)
                assert(pad.sticks[2] == -1024 and pad.triggers[1] == 200)
            else
                assert(input.count() == 0 and input.event(1) == nil)
            end
            transform.set_position(frames, 0, 0)
        end
    "#,
        )
        .unwrap();
    runtime.world.borrow_mut().add_system(|world| {
        assert!(matches!(
            world.frame_input(),
            [] | [
                RawInputEvent::KeyPressed {
                    key: KeyCode::Space
                },
                RawInputEvent::MouseMoved { .. },
                RawInputEvent::GamepadState { .. },
            ]
        ));
    });
    let mut input = RawInputState::new();
    input.push(RawInputEvent::KeyPressed {
        key: KeyCode::Space,
    });
    input.push(RawInputEvent::MouseMoved { x: 32.0, y: 48.0 });
    input.push(RawInputEvent::GamepadState {
        id: 1,
        buttons: 0,
        sticks: [0, -1024, 0, 0],
        triggers: [200, 0],
    });
    runtime.frame(input.drain(), 0.25, 0.25, |_| {}).unwrap();
    assert_eq!(position(&runtime, first), [0.5, 1.0, 0.0]);
    assert_eq!(position(&runtime, second), [1.0, 0.0, 0.0]);
    assert!(input.is_empty());

    runtime.frame(input.drain(), 0.25, 0.50, |_| {}).unwrap();
    assert_eq!(position(&runtime, first), [1.0, 1.0, 0.0]);
    assert_eq!(position(&runtime, second), [2.0, 0.0, 0.0]);
}

#[test]
fn lifecycle_and_event_bus_use_the_same_world() {
    let mut runtime = runtime();
    runtime
        .scripting
        .register_text("level".into(), "ready".into())
        .unwrap();
    let entity = runtime
        .spawn(
            "lifecycle",
            r#"
        local order = 0
        function on_start()
            assert(components.has("ScriptComponent"))
            assert(resources.read_text(resources.find("level")) == "ready")
            assert(resources.find("missing") == nil and resources.read_text(99) == nil)
            order = 1
            events.emit("ready", entity.id())
        end
        function on_update(delta)
            assert(delta == time.delta() and time.elapsed() >= delta and order > 0)
        end
        function on_fixed_update(delta)
            assert(delta > 0)
            order += 1
        end
        function on_collision(other)
            local payload, sender = events.latest("ready")
            assert(payload == entity.id() and sender == entity.id() and other ~= sender)
            transform.set_position(order, 1, 0)
        end
        function on_trigger(other)
            assert(other ~= entity.id())
            local x, y = transform.get_position()
            transform.set_position(x, y, 1)
        end
        function on_destroy()
            events.emit("destroyed", entity.id())
        end
    "#,
        )
        .unwrap();
    {
        let mut world = runtime.world.borrow_mut();
        let other = world.spawn();
        world
            .event_bus_mut()
            .publish(ContactEvent::Collision(entity, other));
        world
            .event_bus_mut()
            .publish(ContactEvent::Trigger(other, entity));
    }
    runtime
        .frame(vec![], FIXED_DELTA, FIXED_DELTA, |_| {})
        .unwrap();
    assert_eq!(position(&runtime, entity), [0.0; 3]);
    runtime
        .frame(vec![], FIXED_DELTA, FIXED_DELTA * 2.0, |world| {
            let transform = world.get_component::<Transform>(entity).unwrap();
            assert_eq!(transform.position, [3.0, 1.0, 1.0]);
        })
        .unwrap();

    assert!(runtime.world.borrow_mut().despawn(entity));
    runtime.frame(vec![], 0.0, 1.0, |_| {}).unwrap();
    let destroyed = runtime
        .previous_events
        .of_type::<tellurium_scripting::ScriptEvent>()
        .filter(|event| event.name == "destroyed")
        .collect::<Vec<_>>();
    assert_eq!(destroyed.len(), 1);
    assert_eq!(destroyed[0].sender, entity);
    assert!(!runtime
        .world
        .borrow()
        .has_component::<ScriptComponent>(entity));
    assert!(runtime.world.borrow().query::<Transform>().next().is_none());

    runtime.frame(vec![], 0.0, 1.0, |_| {}).unwrap();
    assert!(runtime
        .previous_events
        .of_type::<tellurium_scripting::ScriptEvent>()
        .next()
        .is_none());
}

#[test]
fn sandbox_shares_engine_api_but_keeps_private_globals() {
    let mut runtime = runtime();
    let first = runtime
        .spawn(
            "writer",
            r#"
        secret = 42
        function on_update()
            secret += 1
            transform.set_position(secret, 0, 0)
        end
    "#,
        )
        .unwrap();
    let second = runtime
        .spawn(
            "reader",
            r#"
        assert(secret == nil and io == nil and os == nil and require == nil)
        assert(loadstring == nil and package == nil and coroutine == nil)
        assert(table.isfrozen(transform) and table.isfrozen(math))
        function on_update()
            transform.set_position(7, 0, 0)
        end
    "#,
        )
        .unwrap();

    runtime.frame(vec![], 0.0, 0.0, |_| {}).unwrap();
    runtime.frame(vec![], 0.0, 0.0, |_| {}).unwrap();
    assert_eq!(position(&runtime, first)[0], 44.0);
    assert_eq!(position(&runtime, second)[0], 7.0);
}

#[test]
fn execution_and_retained_memory_quotas_isolate_failing_scripts() {
    let mut runtime = Runtime::new(LuauConfig {
        max_memory_bytes: 512 * 1024,
        max_cpu_seconds_per_frame: 0.030,
    })
    .unwrap();

    let looped = runtime
        .spawn(
            "loop",
            r#"
        function on_update()
            while true do end
        end
    "#,
        )
        .unwrap();
    let allocated = runtime
        .spawn(
            "memory",
            r#"
        local blocks = {}
        function on_update()
            table.insert(blocks, buffer.create(160 * 1024))
            transform.set_position(#blocks, 0, 0)
        end
    "#,
        )
        .unwrap();
    let healthy = runtime
        .spawn(
            "healthy",
            r#"
        function on_update()
            local x = transform.get_position()
            transform.set_position(x + 1, 0, 0)
        end
    "#,
        )
        .unwrap();

    for _ in 0..6 {
        runtime.frame(vec![], 0.0, 0.0, |_| {}).unwrap();
    }

    assert_eq!(position(&runtime, looped)[0], 0.0);
    assert_eq!(position(&runtime, allocated)[0], 3.0);
    assert_eq!(position(&runtime, healthy)[0], 6.0);
}
#[cfg(all(feature = "editor", windows))]
mod reload;
#[cfg(all(feature = "editor", windows))]
mod samples;
