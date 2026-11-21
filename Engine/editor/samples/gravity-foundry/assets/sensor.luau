sensor_hits = 0

function on_trigger(other)
    sensor_hits += 1
    events.emit("foundry.gate", other)
end
