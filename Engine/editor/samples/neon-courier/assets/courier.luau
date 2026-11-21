local route = {{-2.6, 1.15}, {0, 1.45}, {2.65, 0.95}, {2.7, -1.2}, {-0.6, -1.32}}
local dash_duration = 0.16
move_x, move_y = 0, 0
facing_x, facing_y = 0, 1
dash_left, dash_cooldown = 0, 0
checkpoint, laps = 1, 0
lap_time, best_time = 0, 0

local function bind_controls()
    input.bind_action("Move", function(x, y)
        move_x, move_y = x, y
        if x * x + y * y > 0.04 then
            local length = math.sqrt(x * x + y * y)
            facing_x, facing_y = x / length, y / length
        end
    end)
    input.bind_action("Dash", function(pressed)
        if pressed and dash_cooldown <= 0 then
            dash_left, dash_cooldown = dash_duration, 0.6
        end
    end)
end

on_start = bind_controls
on_reload = bind_controls

function on_update(delta)
    lap_time += delta
    dash_cooldown = math.max(0, dash_cooldown - delta)
    local x, y, z = transform.get_position()
    if dash_left > 0 then
        local step = math.min(dash_left, delta)
        x += facing_x * 10 * step
        y += facing_y * 10 * step
        dash_left -= step
    else
        local length = math.max(1, math.sqrt(move_x * move_x + move_y * move_y))
        x += move_x * 3.6 * delta / length
        y += move_y * 3.6 * delta / length
    end
    x = math.clamp(x, -3.35, 3.35)
    y = math.clamp(y, -1.78, 1.78)
    transform.set_position(x, y, z)

    local target = route[checkpoint]
    local dx, dy = x - target[1], y - target[2]
    if dx * dx + dy * dy < 0.35 * 0.35 then
        if checkpoint == #route then
            laps += 1
            best_time = if best_time > 0 then math.min(best_time, lap_time) else lap_time
            lap_time = 0
        end
        checkpoint = checkpoint % #route + 1
        events.emit("courier.checkpoint", tostring(checkpoint))
    end
end
