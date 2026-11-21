shuttle_progress = 0
in_motion = false

local function bind_controls()
    input.bind_action("SendShuttle", function(pressed)
        if pressed and not in_motion then
            shuttle_progress = 0
            in_motion = true
        end
    end)
end

on_start = bind_controls
on_reload = bind_controls

function on_fixed_update(delta)
    if not in_motion then
        return
    end
    shuttle_progress = math.min(1, shuttle_progress + delta / 2.8)
    local progress = 1 - math.abs(shuttle_progress * 2 - 1)
    transform.set_position(-2.5 + progress * 5, 0.2, -1.25)
    in_motion = shuttle_progress < 1
end
