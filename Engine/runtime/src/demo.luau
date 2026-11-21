local speed: number = 2
local bound: number = 1.5

function on_start()
    assert(entity.exists() and components.has("Transform"))
end

function on_update(delta: number)
    local x, y, z = transform.get_position()
    x = (x + delta * speed + bound) % (2 * bound) - bound
    for index = 1, input.count() do
        local event = input.event(index)
        if event.kind == "key_pressed" and event.key == "Space" then
            y += 1
        end
    end
    transform.set_position(x, y, z)
end
