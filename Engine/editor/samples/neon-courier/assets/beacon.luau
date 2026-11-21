local route = {{-2.6, 1.15}, {0, 1.45}, {2.65, 0.95}, {2.7, -1.2}, {-0.6, -1.32}}
next_checkpoint = 1

function on_update()
    local message = events.latest("courier.checkpoint")
    if message then
        next_checkpoint = tonumber(message) or next_checkpoint
    end
    local target = route[next_checkpoint]
    transform.set_position(target[1], target[2] + math.sin(time.elapsed() * 8) * 0.1, -0.14)
end
