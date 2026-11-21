#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif

#include "tellurium-bridge/src/ffi.rs.h"
#include "tellurium/profile.hpp"
#include "mesh_shader.hpp"

#include <windows.h>
#include <d3d12.h>
#include <d3dcompiler.h>
#include <dxgi1_4.h>
#include <wrl/client.h>

#include <algorithm>
#include <array>
#include <climits>
#include <cstddef>
#include <cstring>
#include <iterator>
#include <limits>
#include <sstream>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace tellurium {
namespace {

using Microsoft::WRL::ComPtr;
constexpr UINT frames_in_flight = 2;
constexpr UINT max_textures = 1024;
constexpr DXGI_FORMAT color_format = DXGI_FORMAT_R8G8B8A8_UNORM;
constexpr DXGI_FORMAT depth_format = DXGI_FORMAT_D24_UNORM_S8_UINT;
constexpr UINT64 constant_stride = D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT;

void check(HRESULT result, const char* operation) {
    if (FAILED(result)) {
        std::ostringstream message;
        message << operation << " failed (HRESULT 0x" << std::hex << static_cast<std::uint32_t>(result)
                << ')';
        throw std::runtime_error(message.str());
    }
}

void check_size(UINT width, UINT height) {
    if (width == 0 || height == 0 || width > D3D12_REQ_TEXTURE2D_U_OR_V_DIMENSION ||
        height > D3D12_REQ_TEXTURE2D_U_OR_V_DIMENSION) {
        throw std::invalid_argument("invalid DirectX 12 image dimensions");
    }
}

ComPtr<IDXGIFactory4> create_factory() {
    ComPtr<IDXGIFactory4> factory;
    check(CreateDXGIFactory2(0, IID_PPV_ARGS(factory.GetAddressOf())), "CreateDXGIFactory2");
    return factory;
}

ComPtr<ID3D12Device> create_device(IDXGIFactory4& factory) {
    ComPtr<ID3D12Device> device;
    const auto result =
        D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_11_0, IID_PPV_ARGS(device.GetAddressOf()));
    if (FAILED(result)) {
        ComPtr<IDXGIAdapter> warp;
        check(factory.EnumWarpAdapter(IID_PPV_ARGS(warp.GetAddressOf())), "EnumWarpAdapter");
        check(D3D12CreateDevice(warp.Get(), D3D_FEATURE_LEVEL_11_0,
                                IID_PPV_ARGS(device.ReleaseAndGetAddressOf())),
              "D3D12CreateDevice (WARP)");
    }
    return device;
}

std::string utf8(const wchar_t* text) {
    const auto count = WideCharToMultiByte(CP_UTF8, 0, text, -1, nullptr, 0, nullptr, nullptr);
    if (count <= 1) {
        return {};
    }
    std::string converted(static_cast<std::size_t>(count), '\0');
    WideCharToMultiByte(CP_UTF8, 0, text, -1, converted.data(), count, nullptr, nullptr);
    converted.pop_back();
    return converted;
}

std::string describe_adapter(IDXGIFactory4& factory, ID3D12Device& device) {
    ComPtr<IDXGIAdapter1> adapter;
    check(factory.EnumAdapterByLuid(device.GetAdapterLuid(), IID_PPV_ARGS(adapter.GetAddressOf())),
          "EnumAdapterByLuid");
    DXGI_ADAPTER_DESC1 desc{};
    check(adapter->GetDesc1(&desc), "GetDesc1");
    auto name = utf8(desc.Description);
    if (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) {
        name += " (software)";
    }
    return name;
}

D3D12_RESOURCE_DESC texture_desc(UINT width, UINT height, DXGI_FORMAT format) {
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = width;
    desc.Height = height;
    desc.DepthOrArraySize = 1;
    desc.MipLevels = 1;
    desc.Format = format;
    desc.SampleDesc.Count = 1;
    desc.Layout = D3D12_TEXTURE_LAYOUT_UNKNOWN;
    return desc;
}

ComPtr<ID3D12Resource> resource(ID3D12Device& device, const D3D12_RESOURCE_DESC& desc,
                                D3D12_HEAP_TYPE type, D3D12_RESOURCE_STATES state,
                                const D3D12_CLEAR_VALUE* clear = nullptr) {
    D3D12_HEAP_PROPERTIES properties{};
    properties.Type = type;
    properties.CreationNodeMask = 1;
    properties.VisibleNodeMask = 1;
    ComPtr<ID3D12Resource> result;
    check(device.CreateCommittedResource(&properties, D3D12_HEAP_FLAG_NONE, &desc, state, clear,
                                         IID_PPV_ARGS(result.GetAddressOf())),
          "CreateCommittedResource");
    return result;
}

ComPtr<ID3D12DescriptorHeap> descriptor_heap(ID3D12Device& device, D3D12_DESCRIPTOR_HEAP_TYPE type,
                                             UINT count, D3D12_DESCRIPTOR_HEAP_FLAGS flags) {
    D3D12_DESCRIPTOR_HEAP_DESC desc{};
    desc.Type = type;
    desc.NumDescriptors = count;
    desc.Flags = flags;
    ComPtr<ID3D12DescriptorHeap> result;
    check(device.CreateDescriptorHeap(&desc, IID_PPV_ARGS(result.GetAddressOf())),
          "CreateDescriptorHeap");
    return result;
}

D3D12_RESOURCE_BARRIER transition(ID3D12Resource& resource, D3D12_RESOURCE_STATES before,
                                  D3D12_RESOURCE_STATES after) {
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    barrier.Transition.pResource = &resource;
    barrier.Transition.Subresource = D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES;
    barrier.Transition.StateBefore = before;
    barrier.Transition.StateAfter = after;
    return barrier;
}

struct UploadBuffer {
    ComPtr<ID3D12Resource> buffer;
    std::byte* mapped = nullptr;
    UINT64 capacity = 0;

    void reserve(ID3D12Device& device, UINT64 bytes) {
        if (bytes <= capacity) {
            return;
        }
        D3D12_RESOURCE_DESC desc{};
        desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
        desc.Width = bytes;
        desc.Height = 1;
        desc.DepthOrArraySize = 1;
        desc.MipLevels = 1;
        desc.SampleDesc.Count = 1;
        desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
        auto replacement =
            resource(device, desc, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        const D3D12_RANGE no_reads{0, 0};
        void* address = nullptr;
        check(replacement->Map(0, &no_reads, &address), "Map upload buffer");
        // D3D12 permits persistent mapped upload memory. The pointer is only
        // used while the resource is retained and after its frame fence.
        buffer = std::move(replacement);
        mapped = static_cast<std::byte*>(address);
        capacity = bytes;
    }
};

ComPtr<ID3D12CommandAllocator> command_allocator(ID3D12Device& device) {
    ComPtr<ID3D12CommandAllocator> allocator;
    check(device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,
                                        IID_PPV_ARGS(allocator.GetAddressOf())),
          "CreateCommandAllocator");
    return allocator;
}

struct CloseEvent {
    void operator()(void* handle) const noexcept {
        CloseHandle(handle);
    }
};

struct RenderContext {
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12GraphicsCommandList> list;
    ComPtr<ID3D12Fence> fence;
    std::unique_ptr<void, CloseEvent> event;
    ComPtr<ID3D12CommandAllocator> upload_allocator;
    UINT64 next_fence = 1;

    explicit RenderContext(ID3D12Device& device)
        : event(CreateEventW(nullptr, FALSE, FALSE, nullptr)),
          upload_allocator(command_allocator(device)) {
        if (!event) {
            check(HRESULT_FROM_WIN32(GetLastError()), "CreateEventW");
        }
        D3D12_COMMAND_QUEUE_DESC desc{};
        desc.Type = D3D12_COMMAND_LIST_TYPE_DIRECT;
        check(device.CreateCommandQueue(&desc, IID_PPV_ARGS(queue.GetAddressOf())),
              "CreateCommandQueue");
        check(device.CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(fence.GetAddressOf())),
              "CreateFence");
        check(device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, upload_allocator.Get(),
                                       nullptr, IID_PPV_ARGS(list.GetAddressOf())),
              "CreateCommandList");
        check(list->Close(), "Close initial command list");
    }

    void begin(ID3D12CommandAllocator& allocator, ID3D12PipelineState* pipeline = nullptr) {
        check(allocator.Reset(), "Reset command allocator");
        check(list->Reset(&allocator, pipeline), "Reset command list");
    }

    void submit() {
        check(list->Close(), "Close command list");
        ID3D12CommandList* lists[] = {list.Get()};
        queue->ExecuteCommandLists(1, lists);
    }

    UINT64 signal() {
        const auto value = next_fence;
        check(queue->Signal(fence.Get(), value), "Signal render fence");
        ++next_fence;
        return value;
    }

    void wait(UINT64 value) {
        const auto completed = fence->GetCompletedValue();
        if (completed == std::numeric_limits<UINT64>::max()) {
            throw std::runtime_error("DirectX 12 device was removed");
        }
        if (completed >= value) {
            return;
        }
        check(fence->SetEventOnCompletion(value, event.get()), "SetEventOnCompletion");
        const auto result = WaitForSingleObject(event.get(), 10000);
        if (result != WAIT_OBJECT_0) {
            throw std::runtime_error("waiting for the DirectX 12 GPU failed or timed out");
        }
        const auto finished = fence->GetCompletedValue();
        if (finished == std::numeric_limits<UINT64>::max()) {
            throw std::runtime_error("DirectX 12 device was removed while waiting");
        }
        if (finished < value) {
            throw std::runtime_error("DirectX 12 fence was signaled before the submission completed");
        }
    }
};

struct SwapChain {
    ComPtr<IDXGISwapChain3> chain;
    ComPtr<ID3D12DescriptorHeap> rtv_heap;
    ComPtr<ID3D12DescriptorHeap> dsv_heap;
    std::array<ComPtr<ID3D12Resource>, frames_in_flight> targets;
    ComPtr<ID3D12Resource> depth;
    UINT rtv_stride;
    UINT width;
    UINT height;

    SwapChain(IDXGIFactory4& factory, ID3D12Device& device, ID3D12CommandQueue& queue, HWND window,
              UINT width, UINT height)
        : rtv_heap(descriptor_heap(device, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, frames_in_flight,
                                   D3D12_DESCRIPTOR_HEAP_FLAG_NONE)),
          dsv_heap(descriptor_heap(device, D3D12_DESCRIPTOR_HEAP_TYPE_DSV, 1,
                                   D3D12_DESCRIPTOR_HEAP_FLAG_NONE)),
          rtv_stride(device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV)),
          width(width), height(height) {
        check_size(width, height);
        if (!IsWindow(window)) {
            throw std::invalid_argument("invalid HWND for the DirectX 12 swap chain");
        }

        DXGI_SWAP_CHAIN_DESC1 desc{};
        desc.Width = width;
        desc.Height = height;
        desc.Format = color_format;
        desc.SampleDesc.Count = 1;
        desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
        desc.BufferCount = frames_in_flight;
        desc.SwapEffect = DXGI_SWAP_EFFECT_FLIP_DISCARD;
        ComPtr<IDXGISwapChain1> initial;
        check(factory.CreateSwapChainForHwnd(&queue, window, &desc, nullptr, nullptr,
                                             initial.GetAddressOf()),
              "CreateSwapChainForHwnd");
        check(initial.As(&chain), "Query IDXGISwapChain3");
        check(factory.MakeWindowAssociation(window, DXGI_MWA_NO_ALT_ENTER), "MakeWindowAssociation");
        acquire_targets(device);
    }

    D3D12_CPU_DESCRIPTOR_HANDLE rtv(UINT index) const {
        auto view = rtv_heap->GetCPUDescriptorHandleForHeapStart();
        view.ptr += static_cast<SIZE_T>(index) * rtv_stride;
        return view;
    }

    D3D12_CPU_DESCRIPTOR_HANDLE dsv() const {
        return dsv_heap->GetCPUDescriptorHandleForHeapStart();
    }

    void acquire_targets(ID3D12Device& device) {
        for (UINT index = 0; index < frames_in_flight; ++index) {
            check(chain->GetBuffer(index, IID_PPV_ARGS(targets[index].GetAddressOf())), "GetBuffer");
            device.CreateRenderTargetView(targets[index].Get(), nullptr, rtv(index));
        }

        auto desc = texture_desc(width, height, depth_format);
        desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL;
        D3D12_CLEAR_VALUE clear{};
        clear.Format = depth_format;
        clear.DepthStencil.Depth = 1.0f;
        depth =
            resource(device, desc, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_DEPTH_WRITE, &clear);
        D3D12_DEPTH_STENCIL_VIEW_DESC view{};
        view.Format = depth_format;
        view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE2D;
        device.CreateDepthStencilView(depth.Get(), &view, dsv());
    }

    void resize(ID3D12Device& device, UINT new_width, UINT new_height) {
        for (auto& target : targets) {
            target.Reset();
        }
        depth.Reset();
        check(chain->ResizeBuffers(frames_in_flight, new_width, new_height, color_format, 0),
              "ResizeBuffers");
        width = new_width;
        height = new_height;
        acquire_targets(device);
    }
};

void check_shader(HRESULT result, const ComPtr<ID3DBlob>& diagnostics, const char* operation) {
    if (FAILED(result) && diagnostics) {
        const auto* text = static_cast<const char*>(diagnostics->GetBufferPointer());
        throw std::runtime_error(std::string(text, diagnostics->GetBufferSize()));
    }
    check(result, operation);
}

ComPtr<ID3DBlob> compile_shader(rust::Str source, const char* entry, const char* target) {
    ComPtr<ID3DBlob> bytecode;
    ComPtr<ID3DBlob> diagnostics;
    const auto result =
        D3DCompile(source.data(), source.size(), "mesh.hlsl", nullptr, nullptr, entry, target,
                   D3DCOMPILE_ENABLE_STRICTNESS, 0, bytecode.GetAddressOf(), diagnostics.GetAddressOf());
    check_shader(result, diagnostics, "D3DCompile");
    return bytecode;
}

ComPtr<ID3D12RootSignature> create_root_signature(ID3D12Device& device) {
    D3D12_DESCRIPTOR_RANGE color_texture{};
    color_texture.RangeType = D3D12_DESCRIPTOR_RANGE_TYPE_SRV;
    color_texture.NumDescriptors = 1;
    color_texture.BaseShaderRegister = 0;

    D3D12_ROOT_PARAMETER roots[3]{};
    for (UINT index = 0; index < 2; ++index) {
        roots[index].ParameterType = D3D12_ROOT_PARAMETER_TYPE_CBV;
        roots[index].Descriptor.ShaderRegister = index;
        roots[index].ShaderVisibility = D3D12_SHADER_VISIBILITY_VERTEX;
    }
    roots[2].ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    roots[2].DescriptorTable.NumDescriptorRanges = 1;
    roots[2].DescriptorTable.pDescriptorRanges = &color_texture;
    roots[2].ShaderVisibility = D3D12_SHADER_VISIBILITY_PIXEL;

    D3D12_STATIC_SAMPLER_DESC sampler{};
    sampler.Filter = D3D12_FILTER_MIN_MAG_MIP_LINEAR;
    sampler.AddressU = sampler.AddressV = sampler.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
    sampler.MaxAnisotropy = 1;
    sampler.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
    sampler.MaxLOD = D3D12_FLOAT32_MAX;
    sampler.ShaderVisibility = D3D12_SHADER_VISIBILITY_PIXEL;

    D3D12_ROOT_SIGNATURE_DESC desc{};
    desc.NumParameters = static_cast<UINT>(std::size(roots));
    desc.pParameters = roots;
    desc.NumStaticSamplers = 1;
    desc.pStaticSamplers = &sampler;
    desc.Flags = D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT;

    ComPtr<ID3DBlob> signature;
    ComPtr<ID3DBlob> diagnostics;
    const auto result = D3D12SerializeRootSignature(
        &desc, D3D_ROOT_SIGNATURE_VERSION_1, signature.GetAddressOf(), diagnostics.GetAddressOf());
    check_shader(result, diagnostics, "D3D12SerializeRootSignature");
    ComPtr<ID3D12RootSignature> root;
    check(device.CreateRootSignature(0, signature->GetBufferPointer(), signature->GetBufferSize(),
                                     IID_PPV_ARGS(root.GetAddressOf())),
          "CreateRootSignature");
    return root;
}

ComPtr<ID3D12PipelineState> create_pipeline(ID3D12Device& device, ID3D12RootSignature& root,
                                          rust::Str source = rust::Str(mesh_shader)) {
    const auto vertex = compile_shader(source, "vertex_main", "vs_5_0");
    const auto pixel = compile_shader(source, "pixel_main", "ps_5_0");
    const D3D12_INPUT_ELEMENT_DESC elements[] = {
        {"POSITION", 0, DXGI_FORMAT_R32G32B32_FLOAT, 0, offsetof(Vertex, position),
         D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
        {"COLOR", 0, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, offsetof(Vertex, color),
         D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
        {"TEXCOORD", 0, DXGI_FORMAT_R32G32_FLOAT, 0, offsetof(Vertex, uv),
         D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
    };

    D3D12_GRAPHICS_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = &root;
    desc.VS = {vertex->GetBufferPointer(), vertex->GetBufferSize()};
    desc.PS = {pixel->GetBufferPointer(), pixel->GetBufferSize()};
    desc.InputLayout = {elements, static_cast<UINT>(std::size(elements))};
    desc.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
    desc.NumRenderTargets = 1;
    desc.RTVFormats[0] = color_format;
    desc.DSVFormat = depth_format;
    desc.SampleDesc.Count = 1;
    desc.SampleMask = UINT_MAX;
    desc.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
    desc.RasterizerState.CullMode = D3D12_CULL_MODE_NONE;
    desc.RasterizerState.DepthClipEnable = TRUE;

    auto& blend = desc.BlendState.RenderTarget[0];
    blend.SrcBlend = blend.SrcBlendAlpha = D3D12_BLEND_ONE;
    blend.DestBlend = blend.DestBlendAlpha = D3D12_BLEND_ZERO;
    blend.BlendOp = blend.BlendOpAlpha = D3D12_BLEND_OP_ADD;
    blend.LogicOp = D3D12_LOGIC_OP_NOOP;
    blend.RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL;

    auto& depth = desc.DepthStencilState;
    depth.DepthEnable = TRUE;
    depth.DepthWriteMask = D3D12_DEPTH_WRITE_MASK_ALL;
    depth.DepthFunc = D3D12_COMPARISON_FUNC_LESS;
    depth.StencilReadMask = D3D12_DEFAULT_STENCIL_READ_MASK;
    depth.StencilWriteMask = D3D12_DEFAULT_STENCIL_WRITE_MASK;
    depth.FrontFace = depth.BackFace = {
        D3D12_STENCIL_OP_KEEP,
        D3D12_STENCIL_OP_KEEP,
        D3D12_STENCIL_OP_KEEP,
        D3D12_COMPARISON_FUNC_ALWAYS,
    };

    ComPtr<ID3D12PipelineState> pipeline;
    check(device.CreateGraphicsPipelineState(&desc, IID_PPV_ARGS(pipeline.GetAddressOf())),
          "CreateGraphicsPipelineState");
    return pipeline;
}

struct Mesh {
    ComPtr<ID3D12Resource> buffer;
    D3D12_VERTEX_BUFFER_VIEW vertices{};
    D3D12_INDEX_BUFFER_VIEW indices{};
    UINT index_count = 0;
};

struct Frame {
    ComPtr<ID3D12CommandAllocator> allocator;
    UploadBuffer constants;
    UINT64 fence = 0;
};

#ifdef TRACY_ENABLE
struct GpuTimers {
    ComPtr<ID3D12QueryHeap> queries;
    ComPtr<ID3D12Resource> readback;
    std::array<bool, frames_in_flight> pending{};
    UINT64 frequency = 0;

    GpuTimers(ID3D12Device& device, ID3D12CommandQueue& queue) {
        D3D12_QUERY_HEAP_DESC query_desc{};
        query_desc.Type = D3D12_QUERY_HEAP_TYPE_TIMESTAMP;
        query_desc.Count = frames_in_flight * 2;
        if (FAILED(queue.GetTimestampFrequency(&frequency)) || frequency == 0 ||
            FAILED(device.CreateQueryHeap(&query_desc, IID_PPV_ARGS(queries.GetAddressOf())))) {
            return;
        }
        try {
            D3D12_RESOURCE_DESC buffer{};
            buffer.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
            buffer.Width = frames_in_flight * 2 * sizeof(UINT64);
            buffer.Height = 1;
            buffer.DepthOrArraySize = 1;
            buffer.MipLevels = 1;
            buffer.SampleDesc.Count = 1;
            buffer.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
            readback = resource(device, buffer, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        } catch (const std::exception&) {
            // Profiling is optional. A failed query allocation leaves rendering active.
            queries.Reset();
        }
    }

    void collect(UINT index) {
        if (!pending[index]) {
            return;
        }
        pending[index] = false;
        const auto offset = index * 2 * sizeof(UINT64);
        D3D12_RANGE range{offset, offset + 2 * sizeof(UINT64)};
        void* mapped = nullptr;
        if (SUCCEEDED(readback->Map(0, &range, &mapped))) {
            const auto* times = static_cast<const UINT64*>(mapped) + index * 2;
            if (times[1] >= times[0]) {
                TracyPlot("DX12 GPU frame (ms)", double(times[1] - times[0]) * 1000.0 / double(frequency));
            }
            D3D12_RANGE written{0, 0};
            readback->Unmap(0, &written);
        }
    }

    bool begin(ID3D12GraphicsCommandList& list, UINT index) {
        if (!readback || !TracyIsConnected) {
            return false;
        }
        list.EndQuery(queries.Get(), D3D12_QUERY_TYPE_TIMESTAMP, index * 2);
        return true;
    }

    void finish(ID3D12GraphicsCommandList& list, UINT index) {
        list.EndQuery(queries.Get(), D3D12_QUERY_TYPE_TIMESTAMP, index * 2 + 1);
        list.ResolveQueryData(queries.Get(), D3D12_QUERY_TYPE_TIMESTAMP, index * 2, 2,
                              readback.Get(), index * 2 * sizeof(UINT64));
        pending[index] = true;
    }
};
#endif

struct DrawConstants {
    std::array<float, 16> model;
    std::array<float, 4> tint;
};
static_assert(sizeof(DrawConstants) <= constant_stride);

bool visible(const RenderCommand& command) {
    return (command.flags & static_cast<std::uint32_t>(RenderFlag::Visible)) != 0;
}

template <typename T>
const T& lookup(const std::vector<T>& entries, std::uint32_t id, const char* kind) {
    if (id >= entries.size()) {
        throw std::invalid_argument(std::string("invalid ") + kind + " handle");
    }
    return entries[id];
}

} // namespace

struct RenderDevice::Impl {
    ComPtr<IDXGIFactory4> factory = create_factory();
    ComPtr<ID3D12Device> device = create_device(*factory.Get());
    std::string adapter = describe_adapter(*factory.Get(), *device.Get());
    RenderContext context{*device.Get()};
#ifdef TRACY_ENABLE
    GpuTimers gpu_timers{*device.Get(), *context.queue.Get()};
#endif
    SwapChain swap;
    ComPtr<ID3D12RootSignature> root = create_root_signature(*device.Get());
    ComPtr<ID3D12PipelineState> pipeline = create_pipeline(*device.Get(), *root.Get());
    ComPtr<ID3D12DescriptorHeap> srv_heap =
        descriptor_heap(*device.Get(), D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, max_textures,
                        D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE);
    UINT srv_stride = device->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
    std::array<Frame, frames_in_flight> frames;
    std::vector<Mesh> meshes;
    std::vector<ComPtr<ID3D12Resource>> textures;
    std::vector<MaterialData> materials;
    // Keep resources alive through teardown if a submitted upload loses its fence.
    ComPtr<ID3D12Resource> pending_upload;
    ComPtr<ID3D12Resource> pending_texture;
    bool ready = true;

    Impl(HWND window, UINT width, UINT height)
        : swap(*factory.Get(), *device.Get(), *context.queue.Get(), window, width, height) {
        for (auto& frame : frames) {
            frame.allocator = command_allocator(*device.Get());
        }
        textures.reserve(max_textures);
    }

    ~Impl() {
        // Release back buffers, constants and textures only after the queue is idle.
        // A removed device may reject Signal/SetEvent; teardown must still finish.
        try {
            context.wait(context.signal());
        } catch (const std::exception&) {
        }
    }

    void resize(UINT width, UINT height) {
        require_ready();
        if (width == swap.width && height == swap.height) {
            return;
        }
        check_size(width, height);
        ready = false;
        context.wait(context.signal());
        swap.resize(*device.Get(), width, height);
        ready = true;
    }

    MeshHandle create_mesh(rust::Slice<const Vertex> vertices,
                           rust::Slice<const std::uint32_t> indices) {
        require_ready();
        if (vertices.empty() || indices.empty() || indices.size() % 3 != 0 ||
            vertices.size() > UINT_MAX / sizeof(Vertex) ||
            indices.size() > UINT_MAX / sizeof(std::uint32_t)) {
            throw std::invalid_argument("invalid indexed triangle mesh size");
        }
        if (std::any_of(indices.begin(), indices.end(),
                        [vertices](auto index) { return index >= vertices.size(); })) {
            throw std::invalid_argument("mesh index is out of range");
        }
        const auto id = next_id(meshes.size());
        const auto vertex_bytes = static_cast<UINT>(vertices.size() * sizeof(Vertex));
        const auto index_bytes = static_cast<UINT>(indices.size() * sizeof(std::uint32_t));
        UploadBuffer upload;
        upload.reserve(*device.Get(), static_cast<UINT64>(vertex_bytes) + index_bytes);
        std::memcpy(upload.mapped, vertices.data(), vertex_bytes);
        std::memcpy(upload.mapped + vertex_bytes, indices.data(), index_bytes);

        const auto base = upload.buffer->GetGPUVirtualAddress();
        meshes.push_back(Mesh{
            std::move(upload.buffer),
            {base, vertex_bytes, sizeof(Vertex)},
            {base + vertex_bytes, index_bytes, DXGI_FORMAT_R32_UINT},
            static_cast<UINT>(indices.size()),
        });
        return MeshHandle{id};
    }

    TextureHandle load_texture(UINT width, UINT height, rust::Slice<const std::uint8_t> pixels,
                               rust::Str name, const TextureHandle* replace = nullptr) {
        TELLURIUM_ZONE("DX12 texture upload");
        require_ready();
        check_size(width, height);
        const std::size_t row_bytes = static_cast<std::size_t>(width) * 4;
        if (pixels.size() != row_bytes * height) {
            throw std::invalid_argument("RGBA8 texture byte count does not match its dimensions");
        }
        if (replace) {
            lookup(textures, replace->id, "texture");
        } else if (textures.size() == max_textures) {
            throw std::runtime_error("DirectX 12 texture descriptor heap is full");
        }

        const auto desc = texture_desc(width, height, color_format);
        auto texture =
            resource(*device.Get(), desc, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST);
        if (!name.empty() && name.size() <= INT_MAX) {
            const auto length = static_cast<int>(name.size());
            const auto count =
                MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, name.data(), length, nullptr, 0);
            if (count > 0) {
                std::wstring label(static_cast<std::size_t>(count), L'\0');
                MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, name.data(), length, label.data(),
                                    count);
                check(texture->SetName(label.c_str()), "SetName on texture");
            }
        }

        D3D12_PLACED_SUBRESOURCE_FOOTPRINT layout{};
        UINT64 total = 0;
        device->GetCopyableFootprints(&desc, 0, 1, 0, &layout, nullptr, nullptr, &total);
        UploadBuffer upload;
        upload.reserve(*device.Get(), total);
        for (UINT row = 0; row < height; ++row) {
            std::memcpy(upload.mapped + layout.Offset +
                            static_cast<std::size_t>(row) * layout.Footprint.RowPitch,
                        pixels.data() + row * row_bytes, row_bytes);
        }

        D3D12_TEXTURE_COPY_LOCATION source{};
        source.pResource = upload.buffer.Get();
        source.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        source.PlacedFootprint = layout;
        D3D12_TEXTURE_COPY_LOCATION destination{};
        destination.pResource = texture.Get();
        destination.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;

        ready = false;
        pending_upload = std::move(upload.buffer);
        pending_texture = texture;
        context.begin(*context.upload_allocator.Get());
        context.list->CopyTextureRegion(&destination, 0, 0, 0, &source, nullptr);
        const auto barrier = transition(*texture.Get(), D3D12_RESOURCE_STATE_COPY_DEST,
                                        D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE);
        context.list->ResourceBarrier(1, &barrier);
        context.submit();
        context.wait(context.signal());
        pending_upload.Reset();
        pending_texture.Reset();
        ready = true;

        // The upload fence also retires all preceding draw lists using this slot.
        const auto id = replace ? replace->id : static_cast<std::uint32_t>(textures.size());
        auto view = srv_heap->GetCPUDescriptorHandleForHeapStart();
        view.ptr += static_cast<SIZE_T>(id) * srv_stride;
        D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
        srv.Format = color_format;
        srv.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2D;
        srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        srv.Texture2D.MipLevels = 1;
        device->CreateShaderResourceView(texture.Get(), &srv, view);
        if (replace) {
            textures[id] = std::move(texture);
        } else {
            textures.push_back(std::move(texture));
        }
        return TextureHandle{id};
    }

    MaterialHandle create_material(MaterialData material) {
        require_ready();
        lookup(textures, material.texture.id, "texture");
        const auto id = next_id(materials.size());
        materials.push_back(material);
        return MaterialHandle{id};
    }

    void render_frame(const FrameParams& params, rust::Slice<const RenderCommand> commands) {
        TELLURIUM_ZONE("DX12 render");
        require_ready();
        UINT64 draw_count = 0;
        for (const auto& command : commands) {
            if (visible(command)) {
                lookup(meshes, command.mesh.id, "mesh");
                lookup(materials, command.material.id, "material");
                ++draw_count;
            }
        }
        if (draw_count > 65535) {
            throw std::invalid_argument("DirectX 12 frame exceeds the draw command limit");
        }

        const UINT index = swap.chain->GetCurrentBackBufferIndex();
        auto& frame = frames[index];
        ready = false;
        context.wait(frame.fence);
        ready = true;
#ifdef TRACY_ENABLE
        gpu_timers.collect(index);
#endif
        frame.constants.reserve(*device.Get(), constant_stride * (draw_count + 1));
        auto* constants = frame.constants.mapped;
        std::memcpy(constants, params.view_projection.data(), sizeof(params.view_projection));

        ready = false;
        context.begin(*frame.allocator.Get(), pipeline.Get());
        auto& list = *context.list.Get();
#ifdef TRACY_ENABLE
        const auto sample_gpu = gpu_timers.begin(list, index);
#endif
        auto& target = *swap.targets[index].Get();
        const auto to_render =
            transition(target, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET);
        list.ResourceBarrier(1, &to_render);

        const D3D12_VIEWPORT viewport{
            0.0f, 0.0f, static_cast<float>(swap.width), static_cast<float>(swap.height), 0.0f, 1.0f};
        const D3D12_RECT scissor{0, 0, static_cast<LONG>(swap.width), static_cast<LONG>(swap.height)};
        list.RSSetViewports(1, &viewport);
        list.RSSetScissorRects(1, &scissor);
        const auto rtv = swap.rtv(index);
        const auto dsv = swap.dsv();
        list.OMSetRenderTargets(1, &rtv, FALSE, &dsv);
        list.ClearRenderTargetView(rtv, params.clear_color.data(), 0, nullptr);
        list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH | D3D12_CLEAR_FLAG_STENCIL, 1.0f, 0, 0,
                                   nullptr);

        ID3D12DescriptorHeap* heaps[] = {srv_heap.Get()};
        list.SetDescriptorHeaps(1, heaps);
        list.SetGraphicsRootSignature(root.Get());
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        const auto base = frame.constants.buffer->GetGPUVirtualAddress();
        list.SetGraphicsRootConstantBufferView(0, base);

        UINT64 offset = constant_stride;
        for (const auto& command : commands) {
            if (!visible(command)) {
                continue;
            }
            const auto& mesh = meshes[command.mesh.id];
            const auto& material = materials[command.material.id];
            const DrawConstants draw{command.transform, material.tint};
            std::memcpy(constants + offset, &draw, sizeof(draw));
            list.SetGraphicsRootConstantBufferView(1, base + offset);
            offset += constant_stride;
            auto texture = srv_heap->GetGPUDescriptorHandleForHeapStart();
            texture.ptr += static_cast<UINT64>(material.texture.id) * srv_stride;
            list.SetGraphicsRootDescriptorTable(2, texture);
            list.IASetVertexBuffers(0, 1, &mesh.vertices);
            list.IASetIndexBuffer(&mesh.indices);
            list.DrawIndexedInstanced(mesh.index_count, 1, 0, 0, 0);
        }

        const auto to_present =
            transition(target, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PRESENT);
        list.ResourceBarrier(1, &to_present);
#ifdef TRACY_ENABLE
        if (sample_gpu) {
            gpu_timers.finish(list, index);
        }
#endif
        context.submit();
        HRESULT present_result;
        {
            TELLURIUM_ZONE("DXGI Present");
            present_result = swap.chain->Present(1, 0);
        }
        frame.fence = context.signal();
        check(present_result, "Present");
        ready = true;
    }

    void require_ready() const {
        if (!ready) {
            throw std::runtime_error("DirectX 12 device is unavailable after a GPU operation failed");
        }
    }

    static std::uint32_t next_id(std::size_t count) {
        if (count >= std::numeric_limits<std::uint32_t>::max()) {
            throw std::runtime_error("graphics handles exhausted");
        }
        return static_cast<std::uint32_t>(count);
    }
};

RenderDevice::RenderDevice(NativeWindow window, UINT width, UINT height)
    : impl_(std::make_unique<Impl>(reinterpret_cast<HWND>(window.hwnd), width, height)) {}

RenderDevice::~RenderDevice() = default;

rust::String RenderDevice::adapter_name() const {
    return rust::String(impl_->adapter);
}

void RenderDevice::resize(UINT width, UINT height) {
    impl_->resize(width, height);
}

MeshHandle RenderDevice::create_mesh(rust::Slice<const Vertex> vertices,
                                     rust::Slice<const std::uint32_t> indices) {
    return impl_->create_mesh(vertices, indices);
}

TextureHandle RenderDevice::load_texture_rgba8(UINT width, UINT height,
                                               rust::Slice<const std::uint8_t> pixels, rust::Str name) {
    return impl_->load_texture(width, height, pixels, name);
}

MaterialHandle RenderDevice::create_material(MaterialData material) {
    return impl_->create_material(material);
}

void RenderDevice::render_frame(const FrameParams& params, rust::Slice<const RenderCommand> commands) {
    impl_->render_frame(params, commands);
}

void RenderDevice::replace_texture_rgba8(TextureHandle handle, UINT width, UINT height,
                                        rust::Slice<const std::uint8_t> pixels, rust::Str name) {
    impl_->load_texture(width, height, pixels, name, &handle);
}

void RenderDevice::reload_mesh_shader(rust::Str source) {
    TELLURIUM_ZONE("DX12 shader reload");
    impl_->require_ready();
    auto pipeline = create_pipeline(*impl_->device.Get(), *impl_->root.Get(), source);
    // DX12 pipeline state is immutable. Retire the old PSO after queued draws finish.
    impl_->context.wait(impl_->context.signal());
    impl_->pipeline = std::move(pipeline);
}

std::unique_ptr<RenderDevice> create_render_device(NativeWindow window, UINT width, UINT height) {
    return std::make_unique<RenderDevice>(window, width, height);
}

} // namespace tellurium
