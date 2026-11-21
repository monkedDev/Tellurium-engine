#include "tellurium-bridge/src/ffi.rs.h"

#include <stdexcept>

namespace tellurium {
namespace {
[[noreturn]] void unsupported() {
    throw std::runtime_error("the DirectX 12 backend is available on Windows");
}
} // namespace

struct RenderDevice::Impl {};
RenderDevice::RenderDevice(NativeWindow, std::uint32_t, std::uint32_t) {
    unsupported();
}
RenderDevice::~RenderDevice() = default;

rust::String RenderDevice::adapter_name() const {
    return {};
}
void RenderDevice::resize(std::uint32_t, std::uint32_t) {
    unsupported();
}
MeshHandle RenderDevice::create_mesh(rust::Slice<const Vertex>, rust::Slice<const std::uint32_t>) {
    unsupported();
}
TextureHandle RenderDevice::load_texture_rgba8(std::uint32_t, std::uint32_t,
                                               rust::Slice<const std::uint8_t>, rust::Str) {
    unsupported();
}
MaterialHandle RenderDevice::create_material(MaterialData) {
    unsupported();
}
void RenderDevice::replace_texture_rgba8(TextureHandle, std::uint32_t, std::uint32_t,
                                        rust::Slice<const std::uint8_t>, rust::Str) {
    unsupported();
}
void RenderDevice::reload_mesh_shader(rust::Str) {
    unsupported();
}
void RenderDevice::render_frame(const FrameParams&, rust::Slice<const RenderCommand>) {
    unsupported();
}

std::unique_ptr<RenderDevice> create_render_device(NativeWindow window, std::uint32_t width,
                                                   std::uint32_t height) {
    return std::make_unique<RenderDevice>(window, width, height);
}

} // namespace tellurium
