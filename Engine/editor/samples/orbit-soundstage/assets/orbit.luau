angle = 0
fast = false

local function bind_controls()
    input.bind_action("ChangeSpeed", function(pressed)
        if pressed then
            fast = not fast
            events.emit("soundstage.speed", if fast then "fast" else "slow")
        end
    end)
end

on_start = bind_controls
on_reload = bind_controls

function on_update(delta)
    angle += delta * (if fast then 1.75 else 0.65)
    transform.set_position(2.05 * math.cos(angle), 0.72, 2.05 * math.sin(angle))
end
