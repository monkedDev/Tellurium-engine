use super::*;
use std::sync::{Arc, Mutex};
use tellurium_input::ActionEvent;

#[test]
fn project_context_priorities_route_digital_analog_and_pointer_actions_to_luau() {
    let mut runtime = runtime();
    runtime.input = demo::controls().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let collected = Arc::clone(&seen);
    runtime
        .world
        .borrow_mut()
        .event_bus_mut()
        .subscribe::<ActionEvent, _>(move |event| {
            collected.lock().unwrap().push(event.clone());
        });
    let entity = runtime
        .spawn(
            "actions",
            r#"
        function on_start()
            input.bind_action("Jump", function(pressed)
                if pressed then
                    local x, y, z = transform.get_position()
                    transform.set_position(x + 1, y, z)
                end
            end)
            input.bind_action("Move", function(x, y)
                local a, _, c = transform.get_position()
                transform.set_position(a, x + y, c)
            end)
            input.bind_action("Confirm", function(pressed)
                if pressed then
                    local x, y, z = transform.get_position()
                    transform.set_position(x, y, z + 1)
                end
            end)
        end
    "#,
        )
        .unwrap();
    let key = |key| RawInputEvent::KeyPressed { key };
    let release = |key| RawInputEvent::KeyReleased { key };
    runtime
        .frame(
            vec![
                key(KeyCode::Space),
                key(KeyCode::Space),
                release(KeyCode::Space),
                key(KeyCode::D),
                RawInputEvent::MouseMotion { dx: 3.0, dy: -1.0 },
            ],
            0.0,
            0.0,
            |_| {},
        )
        .unwrap();
    assert_eq!(position(&runtime, entity), [1.0, 1.0, 0.0]);
    runtime
        .frame(
            vec![
                key(KeyCode::Tab),
                release(KeyCode::Tab),
                release(KeyCode::D),
            ],
            0.0,
            0.0,
            |_| {},
        )
        .unwrap();
    runtime
        .frame(
            vec![
                key(KeyCode::Space),
                release(KeyCode::Space),
                RawInputEvent::MouseMoved { x: 20.0, y: 34.0 },
            ],
            0.0,
            0.0,
            |_| {},
        )
        .unwrap();
    assert_eq!(position(&runtime, entity), [1.0, 0.0, 1.0]);

    runtime
        .frame(
            vec![RawInputEvent::GamepadState {
                id: 0,
                buttons: 4096,
                sticks: [-32768, 0, 0, 0],
                triggers: [0; 2],
            }],
            0.0,
            0.0,
            |_| {},
        )
        .unwrap();
    assert_eq!(position(&runtime, entity), [1.0, -1.0, 2.0]);
    runtime
        .frame(
            vec![RawInputEvent::GamepadDisconnected { id: 0 }],
            0.0,
            0.0,
            |_| {},
        )
        .unwrap();
    assert_eq!(position(&runtime, entity), [1.0, 0.0, 2.0]);

    let seen = seen.lock().unwrap();
    let has = |name, value| {
        seen.iter()
            .any(|event| event.name == name && event.value == value)
    };
    assert!(has("Look", ActionValue::PassThrough([3.0, -1.0])));
    assert!(has("Pointer", ActionValue::PassThrough([20.0, 34.0])));
    assert!(has("Move", ActionValue::Axis([1.0, 0.0])));
}
