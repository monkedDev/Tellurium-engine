#include "tellurium-bridge/src/ffi.rs.h"
#include "tellurium/profile.hpp"

#include <cmath>
#include <stdexcept>

namespace tellurium {

void sync_physics_bodies(PhysicsWorld& world, rust::Slice<const BodyDesc> bodies) {
    world.synchronize(bodies);
}

PhysicsFrame take_physics_frame(PhysicsWorld& world) {
    return world.take_frame();
}

void step_physics(PhysicsWorld& world, float delta) {
    TELLURIUM_ZONE("Bullet step");
    if (!std::isfinite(delta) || delta <= 0.0f) {
        throw std::invalid_argument("physics step requires positive finite seconds");
    }
    world.step(delta);
}

RayHit raycast(const PhysicsWorld& world, const Ray& ray) {
    if (!std::isfinite(ray.max_distance) || ray.max_distance < 0.0f) {
        throw std::invalid_argument("ray distance must be finite and non-negative");
    }
    for (const auto coordinate : ray.origin) {
        if (!std::isfinite(coordinate)) {
            throw std::invalid_argument("ray origin must be finite");
        }
    }
    float length_squared = 0.0f;
    for (const auto coordinate : ray.direction) {
        length_squared += coordinate * coordinate;
    }
    if (!std::isfinite(length_squared) || std::abs(length_squared - 1.0f) > 0.001f) {
        throw std::invalid_argument("ray direction must be a unit vector");
    }
    return world.cast(ray);
}

} // namespace tellurium
