//
//  CanvasShaderSource.swift
//  Unveil
//

import Metal
import os

/// CanvasShaderSource holds the canvas shaders as Metal source text and builds their pipeline.
///
/// The shaders live in a Swift string, compiled at runtime with `makeLibrary(source:options:)`,
/// because building a `.metal` file needs the Metal Toolchain and this machine does not have it.
/// The price is a few milliseconds when the canvas is created, and a broken shader fails a unit
/// test (CanvasTextureTests) instead of the build. Moving to a `.metal` file later is a file move.
///
/// The vertex shader draws the image as a four-vertex triangle strip with no vertex buffer: the
/// quad's NDC corners arrive as one `float4` (min.xy, max.xy) set with `setVertexBytes`, and each
/// corner is picked from `vertex_id`. Texture row 0 is the image's top row while NDC y points up,
/// so the top of the quad samples v = 0. The fragment shader samples the frame with bilinear
/// filtering and returns it unchanged: the engine's pixels are already display-encoded sRGB, the
/// `rgba8Unorm` texture reads them raw, and the `bgra8Unorm` drawable stores them raw (Metal
/// reorders the channels on store), so no swizzle or transfer function is needed.
// Nonisolated: the canvas and the tests build the pipeline, and the app default is MainActor.
nonisolated enum CanvasShaderSource {

    static let vertexFunctionName   = "canvasVertex"
    static let fragmentFunctionName = "canvasFragment"

    static let text = """
        #include <metal_stdlib>
        using namespace metal;

        struct CanvasVertex {
            float4 position [[position]];
            float2 textureCoordinate;
        };

        vertex CanvasVertex canvasVertex(
            uint            vertexID [[vertex_id]],
            constant float4 &quad    [[buffer(0)]]
        ) {
            // Strip order: bottom-left, bottom-right, top-left, top-right.
            float2 corner = float2(float(vertexID & 1u), float(vertexID >> 1u));

            CanvasVertex out;
            out.position          = float4(mix(quad.xy, quad.zw, corner), 0.0, 1.0);
            out.textureCoordinate = float2(corner.x, 1.0 - corner.y);
            return out;
        }

        fragment float4 canvasFragment(
            CanvasVertex      in    [[stage_in]],
            texture2d<float>  image [[texture(0)]]
        ) {
            constexpr sampler bilinear(filter::linear, address::clamp_to_edge);
            return image.sample(bilinear, in.textureCoordinate);
        }
        """

    /// makePipelineState(device:pixelFormat:) compiles `text` on `device` and returns the render
    /// pipeline that draws into a drawable of `pixelFormat`. It throws the compiler's own error when
    /// the source does not compile, and CanvasShaderError when a named function is missing.
    /// The whole build is the `ShaderCompile` signpost: it runs on main when the canvas is made.
    static func makePipelineState(
        device     : some MTLDevice,
        pixelFormat: MTLPixelFormat
    ) throws -> any MTLRenderPipelineState {
        let signpost = Signposts.signposter.beginInterval(
            "ShaderCompile",
            id: Signposts.signposter.makeSignpostID()
        )
        defer { Signposts.signposter.endInterval("ShaderCompile", signpost) }

        let library = try device.makeLibrary(source: text, options: nil)

        guard let vertexFunction = library.makeFunction(name: vertexFunctionName) else {
            throw CanvasShaderError.missingFunction(vertexFunctionName)
        }

        guard let fragmentFunction = library.makeFunction(name: fragmentFunctionName) else {
            throw CanvasShaderError.missingFunction(fragmentFunctionName)
        }

        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction                  = vertexFunction
        descriptor.fragmentFunction                = fragmentFunction
        descriptor.colorAttachments[0].pixelFormat = pixelFormat

        return try device.makeRenderPipelineState(descriptor: descriptor)
    }
}
