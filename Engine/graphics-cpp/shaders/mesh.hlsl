cbuffer FrameConstants : register(b0) {
    column_major float4x4 view_projection;
};

cbuffer DrawConstants : register(b1) {
    column_major float4x4 model;
    float4 tint;
};

Texture2D color_map : register(t0);
SamplerState color_sampler : register(s0);

struct VertexInput {
    float3 position : POSITION;
    float4 color : COLOR0;
    float2 uv : TEXCOORD0;
};

struct PixelInput {
    float4 position : SV_POSITION;
    float4 color : COLOR0;
    float2 uv : TEXCOORD0;
};

PixelInput vertex_main(VertexInput input) {
    PixelInput output;
    output.position = mul(view_projection, mul(model, float4(input.position, 1.0)));
    output.color = input.color * tint;
    output.uv = input.uv;
    return output;
}

float4 pixel_main(PixelInput input) : SV_TARGET {
    return input.color * color_map.Sample(color_sampler, input.uv);
}
