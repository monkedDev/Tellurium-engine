#pragma once

#include "rust/cxx.h"

#include <memory>

namespace tellurium {

struct BodyDesc;
struct PhysicsFrame;
struct Ray;
struct RayHit;

class PhysicsWorld {
  public:
    virtual ~PhysicsWorld() = default;
    virtual void synchronize(rust::Slice<const BodyDesc> bodies) = 0;
    virtual void step(float delta) = 0;
    virtual PhysicsFrame take_frame() = 0;
    virtual RayHit cast(const Ray& ray) const = 0;
};

std::unique_ptr<PhysicsWorld> create_physics_world(float gravity_y);
void sync_physics_bodies(PhysicsWorld& world, rust::Slice<const BodyDesc> bodies);
void step_physics(PhysicsWorld& world, float delta);
PhysicsFrame take_physics_frame(PhysicsWorld& world);
RayHit raycast(const PhysicsWorld& world, const Ray& ray);

} // namespace tellurium
