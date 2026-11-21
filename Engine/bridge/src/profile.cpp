#include "tellurium-bridge/src/ffi.rs.h"

#include "tracy/TracyC.h"

namespace tellurium {
TraceToken trace_begin(std::uint8_t stage) noexcept {
    static const ___tracy_source_location_data locations[] = {
        {"Frame", "Rust", "runtime", 0, 0},
        {"Input", "Rust", "runtime", 0, 0},
        {"Pre-update", "Rust", "runtime", 0, 0},
        {"ECS update", "Rust", "runtime", 0, 0},
        {"Physics synchronization", "Rust", "runtime", 0, 0},
        {"Render preparation", "Rust", "runtime", 0, 0},
        {"Render and present", "Rust", "runtime", 0, 0},
        {"Post-frame", "Rust", "runtime", 0, 0},
        {"Hot reload", "Rust", "runtime", 0, 0},
    };
    if (stage < sizeof(locations) / sizeof(locations[0])) {
        const auto ctx = ___tracy_emit_zone_begin(&locations[stage], 1);
        return {ctx.id, ctx.active};
    }
    return {0, 0};
}

void trace_end(TraceToken token) noexcept {
    ___tracy_emit_zone_end({token.id, token.active});
}

void trace_frame() noexcept {
    FrameMark;
}
}
