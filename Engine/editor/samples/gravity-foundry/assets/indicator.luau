lit_until = 0

function on_update()
    local now = time.elapsed()
    if events.latest("foundry.gate") then
        lit_until = now + 1.25
    end
    local height = if now < lit_until then 1.08 + 0.1 * math.sin(now * 18) else 0.25
    transform.set_position(1.35, height, -1.68)
end
