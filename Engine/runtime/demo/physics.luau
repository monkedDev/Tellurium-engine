local motion_x = 0
local motion_z = 0

function on_start()
    input.bind_action("Jump", function(pressed)
        if not pressed then
            return
        end
        local x, y, z = transform.get_position()
        if y < 0.15 then
            transform.set_position(x, y + 0.85, z)
            events.emit("demo.jump", "")
        end
    end)

    input.bind_action("Move", function(x, z)
        motion_x, motion_z = x, z
    end)

    input.bind_action("Shoot", function(pressed)
        if pressed then
            events.emit("demo.shoot", "")
        end
    end)

    input.bind_action("Confirm", function(pressed)
        if pressed then
            events.emit("demo.confirm", "")
        end
    end)
end

function on_update(delta)
    if motion_x == 0 and motion_z == 0 then
        return
    end
    local x, y, z = transform.get_position()
    transform.set_position(
        math.clamp(x + motion_x * delta * 1.7, -1.7, 1.7),
        y,
        math.clamp(z + motion_z * delta * 1.7, -0.85, 0.85)
    )
end

function on_collision(other)
    events.emit("demo.collision", other)
end

function on_trigger(other)
    events.emit("demo.trigger", other)
end
