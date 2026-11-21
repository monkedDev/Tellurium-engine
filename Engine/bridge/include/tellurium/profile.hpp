#pragma once

#include <cstdint>

#ifdef TRACY_ENABLE
#include "tracy/Tracy.hpp"
#define TELLURIUM_ZONE(name) ZoneScopedN(name)
#else
#define TELLURIUM_ZONE(name) (void)0
#endif

namespace tellurium {
struct TraceToken;
TraceToken trace_begin(std::uint8_t stage) noexcept;
void trace_end(TraceToken token) noexcept;
void trace_frame() noexcept;
}
