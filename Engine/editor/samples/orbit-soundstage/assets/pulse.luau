angle = math.pi
fast = false

function on_update(delta)
    local speed = events.latest("soundstage.speed")
    if speed then
        fast = speed == "fast"
    end
    angle += delta * (if fast then 1.75 else 0.65)
    transform.set_position(1.45 * math.cos(angle), 0.62 + math.sin(angle * 2) * 0.14, 1.45 * math.sin(angle))
end
