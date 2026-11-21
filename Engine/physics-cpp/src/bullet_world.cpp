#include "tellurium-bridge/src/ffi.rs.h"

#include "btBulletDynamicsCommon.h"

#include <algorithm>
#include <cmath>
#include <map>
#include <set>
#include <stdexcept>
#include <tuple>
#include <utility>

namespace tellurium {
namespace {

btVector3 vector(const std::array<float, 3>& values) {
    return {values[0], values[1], values[2]};
}

std::array<float, 3> array(const btVector3& value) {
    return {value.x(), value.y(), value.z()};
}

std::array<float, 4> array(const btQuaternion& value) {
    return {value.x(), value.y(), value.z(), value.w()};
}

template <std::size_t N> bool finite(const std::array<float, N>& values) {
    return std::all_of(values.begin(), values.end(), [](float value) { return std::isfinite(value); });
}

btTransform transform(const Transform& pose) {
    const auto& r = pose.rotation;
    btQuaternion rotation{r[0], r[1], r[2], r[3]};
    return btTransform(rotation.normalized(), vector(pose.position));
}

btVector3 size(const BodyDesc& body) {
    const auto scale = vector(body.pose.scale).absolute();
    if (body.collider == ColliderKind::Sphere) {
        const auto radius = body.dimensions[0] * std::max({scale.x(), scale.y(), scale.z()});
        return {radius, radius, radius};
    }
    return vector(body.dimensions) * scale;
}

void validate(const BodyDesc& body) {
    if (body.entity == 0 || !finite(body.pose.position) || !finite(body.pose.rotation) ||
        !finite(body.pose.scale) || !finite(body.linear_velocity) || !finite(body.dimensions) ||
        !std::isfinite(body.mass) || body.mass < 0.0f ||
        (body.kind == BodyKind::Dynamic && body.mass <= 0.0f)) {
        throw std::invalid_argument("invalid Bullet body descriptor");
    }
    if (body.kind != BodyKind::Dynamic && body.kind != BodyKind::Static &&
        body.kind != BodyKind::Kinematic) {
        throw std::invalid_argument("unknown Bullet body kind");
    }
    if (body.collider != ColliderKind::Box && body.collider != ColliderKind::Sphere) {
        throw std::invalid_argument("unknown Bullet collider");
    }
    const auto& r = body.pose.rotation;
    const auto length = btQuaternion(r[0], r[1], r[2], r[3]).length2();
    const auto extents = size(body);
    if (!std::isfinite(length) || length < 1.0e-8f || !finite(array(extents)) || extents.x() <= 0.0f ||
        extents.y() <= 0.0f || extents.z() <= 0.0f) {
        throw std::invalid_argument("Bullet collider requires a rotation and positive dimensions");
    }
}

using Signature = std::tuple<BodyKind, ColliderKind, std::array<float, 3>, float, bool>;

Signature signature(const BodyDesc& desc) {
    return {desc.kind, desc.collider, array(size(desc)), desc.mass, desc.trigger};
}

struct Body {
    std::uint64_t entity;
    Signature properties;
    std::unique_ptr<btCollisionShape> shape;
    std::unique_ptr<btRigidBody> rigid;

    explicit Body(const BodyDesc& desc) : entity(desc.entity), properties(signature(desc)) {
        const auto extents = size(desc);
        if (desc.collider == ColliderKind::Sphere) {
            shape = std::make_unique<btSphereShape>(extents.x());
        } else {
            shape = std::make_unique<btBoxShape>(extents);
        }
        const auto mass = desc.kind == BodyKind::Dynamic ? desc.mass : 0.0f;
        btVector3 inertia(0, 0, 0);
        if (mass > 0.0f) {
            shape->calculateLocalInertia(mass, inertia);
        }
        btRigidBody::btRigidBodyConstructionInfo info(mass, nullptr, shape.get(), inertia);
        info.m_startWorldTransform = transform(desc.pose);
        info.m_restitution = 0.0f;
        rigid = std::make_unique<btRigidBody>(info);
        rigid->setUserPointer(this);
        auto flags = rigid->getCollisionFlags();
        if (desc.kind == BodyKind::Kinematic) {
            flags &= ~btCollisionObject::CF_STATIC_OBJECT;
            flags |= btCollisionObject::CF_KINEMATIC_OBJECT;
            rigid->setActivationState(DISABLE_DEACTIVATION);
        }
        if (desc.trigger) {
            flags |= btCollisionObject::CF_NO_CONTACT_RESPONSE;
        }
        rigid->setCollisionFlags(flags);
        if (desc.kind == BodyKind::Dynamic) {
            rigid->setLinearVelocity(vector(desc.linear_velocity));
        }
    }

    bool trigger() const {
        return (rigid->getCollisionFlags() & btCollisionObject::CF_NO_CONTACT_RESPONSE) != 0;
    }
};

using Pair = std::tuple<std::uint64_t, std::uint64_t, bool>;

class BulletWorld final : public PhysicsWorld {
    btDefaultCollisionConfiguration configuration;
    btCollisionDispatcher dispatcher{&configuration};
    btDbvtBroadphase broadphase;
    btSequentialImpulseConstraintSolver solver;
    btDiscreteDynamicsWorld dynamics{&dispatcher, &broadphase, &solver, &configuration};
    std::map<std::uint64_t, std::unique_ptr<Body>> bodies;
    std::set<Pair> active_pairs;
    rust::Vec<PhysicsContact> pending_contacts;

  public:
    explicit BulletWorld(float gravity) {
        dynamics.setGravity({0, gravity, 0});
    }

    ~BulletWorld() override {
        for (auto& [id, body] : bodies) {
            dynamics.removeRigidBody(body->rigid.get());
        }
    }

    void synchronize(rust::Slice<const BodyDesc> current) override {
        std::map<std::uint64_t, const BodyDesc*> descriptions;
        for (const auto& desc : current) {
            validate(desc);
            if (!descriptions.emplace(desc.entity, &desc).second) {
                throw std::invalid_argument("duplicate ECS entity in Bullet snapshot");
            }
        }
        for (auto it = bodies.begin(); it != bodies.end();) {
            const auto desc = descriptions.find(it->first);
            if (desc == descriptions.end() || it->second->properties != signature(*desc->second)) {
                dynamics.removeRigidBody(it->second->rigid.get());
                it = bodies.erase(it);
            } else {
                ++it;
            }
        }

        for (const auto& [id, desc] : descriptions) {
            const auto existing = bodies.find(id);
            if (existing == bodies.end()) {
                auto body = std::make_unique<Body>(*desc);
                auto& added = bodies.emplace(id, std::move(body)).first->second;
                dynamics.addRigidBody(added->rigid.get());
                continue;
            }

            auto& rigid = *existing->second->rigid;
            if (desc->override_pose) {
                const auto pose = transform(desc->pose);
                rigid.setWorldTransform(pose);
                if (desc->kind != BodyKind::Kinematic) {
                    rigid.setInterpolationWorldTransform(pose);
                }
                rigid.activate(true);
                dynamics.updateSingleAabb(&rigid);
            }
            if (desc->override_velocity && desc->kind == BodyKind::Dynamic) {
                rigid.setLinearVelocity(vector(desc->linear_velocity));
                rigid.activate(true);
            }
        }
    }

    void step(float delta) override {
        dynamics.stepSimulation(delta, 0, delta);
        std::set<Pair> touching;
        for (int index = 0; index < dispatcher.getNumManifolds(); ++index) {
            auto* manifold = dispatcher.getManifoldByIndexInternal(index);
            for (int p = 0; p < manifold->getNumContacts(); ++p) {
                if (manifold->getContactPoint(p).getDistance() > 0) {
                    continue;
                }
                const auto& a = *static_cast<const Body*>(manifold->getBody0()->getUserPointer());
                const auto& b = *static_cast<const Body*>(manifold->getBody1()->getUserPointer());
                auto [first, second] = std::minmax(a.entity, b.entity);
                touching.emplace(first, second, a.trigger() || b.trigger());
                break;
            }
        }
        for (const auto& pair : touching) {
            if (active_pairs.count(pair) == 0) {
                const auto& [first, second, trigger] = pair;
                pending_contacts.push_back({first, second, trigger});
            }
        }
        active_pairs.swap(touching);
    }

    PhysicsFrame take_frame() override {
        PhysicsFrame frame;
        frame.bodies.reserve(bodies.size());
        for (const auto& [id, body] : bodies) {
            if (std::get<0>(body->properties) != BodyKind::Dynamic) {
                continue;
            }
            const auto& pose = body->rigid->getWorldTransform();
            frame.bodies.push_back({id, array(pose.getOrigin()), array(pose.getRotation()),
                                    array(body->rigid->getLinearVelocity())});
        }
        frame.contacts = std::move(pending_contacts);
        return frame;
    }

    RayHit cast(const Ray& ray) const override {
        const auto origin = vector(ray.origin);
        const auto endpoint = origin + vector(ray.direction) * ray.max_distance;
        btCollisionWorld::ClosestRayResultCallback result(origin, endpoint);
        dynamics.rayTest(origin, endpoint, result);
        if (!result.hasHit()) {
            return {};
        }
        const auto& body = *static_cast<const Body*>(result.m_collisionObject->getUserPointer());
        return {true, body.entity, ray.max_distance * result.m_closestHitFraction,
                array(result.m_hitNormalWorld)};
    }
};

} // namespace

std::unique_ptr<PhysicsWorld> create_physics_world(float gravity_y) {
    if (!std::isfinite(gravity_y)) {
        throw std::invalid_argument("Bullet gravity must be finite");
    }
    return std::make_unique<BulletWorld>(gravity_y);
}

} // namespace tellurium
