#pragma once

#include "rust/cxx.h"

#include <cstdint>
#include <memory>

namespace tellurium {

struct FrameParams;
struct MaterialData;
struct MaterialHandle;
struct MeshHandle;
struct NativeWindow;
struct RenderCommand;
struct TextureHandle;
struct Vertex;

class RenderDevice {
  public:
    RenderDevice(NativeWindow window, std::uint32_t width, std::uint32_t height);
    ~RenderDevice();

    rust::String adapter_name() const;
    void resize(std::uint32_t width, std::uint32_t height);
    MeshHandle create_mesh(rust::Slice<const Vertex> vertices, rust::Slice<const std::uint32_t> indices);
    TextureHandle load_texture_rgba8(std::uint32_t width, std::uint32_t height,
                                     rust::Slice<const std::uint8_t> pixels, rust::Str name);
    MaterialHandle create_material(MaterialData material);
    void replace_texture_rgba8(TextureHandle handle, std::uint32_t width, std::uint32_t height,
                              rust::Slice<const std::uint8_t> pixels, rust::Str name);
    void reload_mesh_shader(rust::Str source);
    void render_frame(const FrameParams& params, rust::Slice<const RenderCommand> commands);

  private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

std::unique_ptr<RenderDevice> create_render_device(NativeWindow window, std::uint32_t width,
                                                   std::uint32_t height);

} // namespace tellurium
