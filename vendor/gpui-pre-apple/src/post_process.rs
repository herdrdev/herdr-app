//! HERDR: full-window post-processing for the Metal renderer.
//!
//! Each shader is a GLSL 450 fragment shader, translated to MSL with naga and
//! run over the finished frame, one pass after another; the last pass writes
//! the drawable. The frame is the shader's only texture (bound to slot 0 and
//! sampled with an inline linear, clamp-to-edge sampler) and its uniform block
//! is filled by member name, at the offsets naga laid it out with.

use anyhow::{Context as _, Result, anyhow};
use gpui::{DevicePixels, ShaderValue, Size};
use metal::{MTLPixelFormat, MTLResourceOptions, MTLStorageMode, MTLTextureUsage};
use naga::back::msl;
use std::collections::HashMap;

/// A full-screen triangle; the fragment shader sees `gl_FragCoord` in pixels.
const VERTEX: &str = r#"
struct HerdrPostVertexOut { metal::float4 position [[position]]; };
vertex HerdrPostVertexOut herdr_post_vertex(unsigned int vid [[vertex_id]]) {
    metal::float2 p = metal::float2(float((vid << 1) & 2u), float(vid & 2u));
    HerdrPostVertexOut out;
    out.position = metal::float4(p * 2.0 - 1.0, 0.0, 1.0);
    return out;
}
"#;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Float,
    Int,
    Vector(usize),
    Array { width: usize, len: usize, stride: usize },
}

#[derive(Clone, Copy, Debug)]
struct Member {
    offset: usize,
    kind: Kind,
}

struct Pass {
    pipeline: metal::RenderPipelineState,
    members: HashMap<String, Member>,
    uniforms: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct PostProcess {
    passes: Vec<Pass>,
    /// The frame, and a second target for chains of more than one pass.
    targets: [Option<metal::Texture>; 2],
}

impl PostProcess {
    pub(crate) fn is_active(&self) -> bool {
        !self.passes.is_empty()
    }

    /// Compiles every shader; on any error nothing changes.
    pub(crate) fn set_shaders(&mut self, device: &metal::DeviceRef, shaders: &[String]) -> Result<()> {
        let passes = shaders
            .iter()
            .enumerate()
            .map(|(i, glsl)| compile(device, glsl).with_context(|| format!("shader {}", i + 1)))
            .collect::<Result<Vec<_>>>()?;
        self.passes = passes;
        if self.passes.is_empty() {
            self.targets = [None, None];
        }
        Ok(())
    }

    pub(crate) fn set_values(&mut self, values: &[(&str, ShaderValue)]) {
        for pass in &mut self.passes {
            for (name, value) in values {
                if let Some(member) = pass.members.get(*name) {
                    write(&mut pass.uniforms, *member, value);
                }
            }
        }
    }

    /// Where the scene should be drawn this frame.
    pub(crate) fn frame_target(&mut self, device: &metal::DeviceRef, size: Size<DevicePixels>) -> metal::Texture {
        target(&mut self.targets[0], device, size)
    }

    /// Runs the passes from the frame target into `drawable`.
    pub(crate) fn encode(
        &mut self,
        device: &metal::DeviceRef,
        command_buffer: &metal::CommandBufferRef,
        drawable: &metal::TextureRef,
        size: Size<DevicePixels>,
    ) {
        let Some(frame) = self.targets[0].clone() else { return };
        let spare = (self.passes.len() > 1).then(|| target(&mut self.targets[1], device, size));
        let mut source = frame.clone();
        let count = self.passes.len();
        for (i, pass) in self.passes.iter().enumerate() {
            let last = i + 1 == count;
            let destination: metal::Texture = if last {
                drawable.to_owned()
            } else if i % 2 == 0 {
                spare.clone().unwrap_or_else(|| frame.clone())
            } else {
                frame.clone()
            };
            let descriptor = metal::RenderPassDescriptor::new();
            let attachment = descriptor.color_attachments().object_at(0).unwrap();
            attachment.set_texture(Some(&destination));
            attachment.set_load_action(metal::MTLLoadAction::DontCare);
            attachment.set_store_action(metal::MTLStoreAction::Store);
            let encoder = command_buffer.new_render_command_encoder(descriptor);
            encoder.set_viewport(metal::MTLViewport {
                originX: 0.0,
                originY: 0.0,
                width: i32::from(size.width) as f64,
                height: i32::from(size.height) as f64,
                znear: 0.0,
                zfar: 1.0,
            });
            encoder.set_render_pipeline_state(&pass.pipeline);
            encoder.set_fragment_texture(0, Some(&source));
            if !pass.uniforms.is_empty() {
                // A fresh buffer each frame: frames in flight keep their own values.
                let buffer = device.new_buffer_with_data(
                    pass.uniforms.as_ptr().cast(),
                    pass.uniforms.len() as u64,
                    MTLResourceOptions::StorageModeShared,
                );
                encoder.set_fragment_buffer(0, Some(&buffer), 0);
            }
            encoder.draw_primitives(metal::MTLPrimitiveType::Triangle, 0, 3);
            encoder.end_encoding();
            source = destination;
        }
    }
}

fn target(slot: &mut Option<metal::Texture>, device: &metal::DeviceRef, size: Size<DevicePixels>) -> metal::Texture {
    let (width, height) = (i32::from(size.width).max(1) as u64, i32::from(size.height).max(1) as u64);
    if let Some(texture) = slot.as_ref().filter(|t| t.width() == width && t.height() == height) {
        return texture.clone();
    }
    let descriptor = metal::TextureDescriptor::new();
    descriptor.set_width(width);
    descriptor.set_height(height);
    descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
    descriptor.set_storage_mode(MTLStorageMode::Private);
    descriptor.set_usage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
    let texture = device.new_texture(&descriptor);
    *slot = Some(texture.clone());
    texture
}

fn compile(device: &metal::DeviceRef, glsl: &str) -> Result<Pass> {
    let mut frontend = naga::front::glsl::Frontend::default();
    let options = naga::front::glsl::Options::from(naga::ShaderStage::Fragment);
    let module = frontend.parse(&options, glsl).map_err(|e| anyhow!("{}", e.emit_to_string(glsl)))?;
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
        .map_err(|e| anyhow!("{}", e.emit_to_string(glsl)))?;
    let entry = module
        .entry_points
        .iter()
        .find(|e| e.stage == naga::ShaderStage::Fragment)
        .context("no fragment entry point")?;

    // The frame texture at slot 0, the uniform block at buffer 0, samplers inline.
    let mut resources = msl::BindingMap::default();
    let mut block = None;
    for (_, global) in module.global_variables.iter() {
        let Some(binding) = global.binding.clone() else { continue };
        let target = match (&module.types[global.ty].inner, global.space) {
            (naga::TypeInner::Image { .. }, _) => msl::BindTarget { texture: Some(0), ..Default::default() },
            (naga::TypeInner::Sampler { .. }, _) => {
                msl::BindTarget { sampler: Some(msl::BindSamplerTarget::Inline(0)), ..Default::default() }
            }
            (_, naga::AddressSpace::Uniform) => {
                anyhow::ensure!(block.is_none(), "only one uniform block is supported");
                block = Some(global.ty);
                msl::BindTarget { buffer: Some(0), ..Default::default() }
            }
            _ => continue,
        };
        resources.insert(binding, target);
    }
    let mut per_entry_point_map = msl::EntryPointResourceMap::default();
    per_entry_point_map.insert(entry.name.clone(), msl::EntryPointResources { resources, ..Default::default() });
    let options = msl::Options {
        lang_version: (2, 1),
        per_entry_point_map,
        inline_samplers: vec![msl::sampler::InlineSampler {
            mag_filter: msl::sampler::Filter::Linear,
            min_filter: msl::sampler::Filter::Linear,
            ..Default::default()
        }],
        fake_missing_bindings: false,
        ..Default::default()
    };
    let (source, translation) = msl::write_string(&module, &info, &options, &msl::PipelineOptions::default())
        .map_err(|e| anyhow!("translating to Metal: {e}"))?;
    let index = module.entry_points.iter().position(|e| e.stage == naga::ShaderStage::Fragment).unwrap_or(0);
    let fragment_name = translation.entry_point_names[index].clone().map_err(|e| anyhow!("translating to Metal: {e}"))?;

    let library = device
        .new_library_with_source(&format!("{source}\n{VERTEX}"), &metal::CompileOptions::new())
        .map_err(|e| anyhow!("compiling for Metal: {e}"))?;
    let vertex = library.get_function("herdr_post_vertex", None).map_err(|e| anyhow!(e))?;
    let fragment = library.get_function(&fragment_name, None).map_err(|e| anyhow!(e))?;
    let descriptor = metal::RenderPipelineDescriptor::new();
    descriptor.set_label("post_process");
    descriptor.set_vertex_function(Some(&vertex));
    descriptor.set_fragment_function(Some(&fragment));
    let attachment = descriptor.color_attachments().object_at(0).unwrap();
    attachment.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
    attachment.set_blending_enabled(false);
    let pipeline = device.new_render_pipeline_state(&descriptor).map_err(|e| anyhow!(e))?;

    let (members, size) = match block {
        Some(ty) => layout(&module, ty),
        None => (HashMap::new(), 0),
    };
    Ok(Pass { pipeline, members, uniforms: vec![0; size] })
}

/// Each member's offset and kind in the uniform block, and the block's size.
fn layout(module: &naga::Module, ty: naga::Handle<naga::Type>) -> (HashMap<String, Member>, usize) {
    let naga::TypeInner::Struct { members, span } = &module.types[ty].inner else {
        return (HashMap::new(), 0);
    };
    let kind_of = |ty: naga::Handle<naga::Type>| -> Option<Kind> {
        match &module.types[ty].inner {
            naga::TypeInner::Scalar(s) if s.kind == naga::ScalarKind::Float => Some(Kind::Float),
            naga::TypeInner::Scalar(_) => Some(Kind::Int),
            naga::TypeInner::Vector { size, .. } => Some(Kind::Vector(*size as usize)),
            naga::TypeInner::Array { base, size: naga::ArraySize::Constant(len), stride } => {
                match &module.types[*base].inner {
                    naga::TypeInner::Vector { size, .. } => {
                        Some(Kind::Array { width: *size as usize, len: len.get() as usize, stride: *stride as usize })
                    }
                    // Packed in Metal whatever the GLSL stride (see below).
                    naga::TypeInner::Scalar(s) => Some(Kind::Array { width: 1, len: len.get() as usize, stride: s.width as usize }),
                    _ => None,
                }
            }
            _ => None,
        }
    };
    // naga lays a uniform block out std140, where `float a[4]` has a 16-byte
    // stride, but writes the Metal array as `float inner[4]` (4 bytes apart)
    // and pads to its own offsets after it. Everything after such an array
    // sits earlier in Metal than naga says, by what the array lost.
    let mut shrink = 0;
    let mut map = HashMap::new();
    for m in members {
        let offset = m.offset as usize - shrink;
        if let naga::TypeInner::Array { base, size: naga::ArraySize::Constant(len), stride } = &module.types[m.ty].inner
            && let naga::TypeInner::Scalar(s) = &module.types[*base].inner
        {
            shrink += (*stride as usize).saturating_sub(s.width as usize) * len.get() as usize;
        }
        if let (Some(name), Some(kind)) = (m.name.clone(), kind_of(m.ty)) {
            map.insert(name, Member { offset, kind });
        }
    }
    (map, (*span as usize).saturating_sub(shrink))
}

fn put(buffer: &mut [u8], offset: usize, bytes: [u8; 4]) {
    if let Some(slot) = buffer.get_mut(offset..offset + 4) {
        slot.copy_from_slice(&bytes);
    }
}

fn write(buffer: &mut [u8], member: Member, value: &ShaderValue) {
    let floats = |buffer: &mut [u8], at: usize, values: &[f32], width: usize| {
        for (i, v) in values.iter().take(width).enumerate() {
            put(buffer, at + i * 4, v.to_le_bytes());
        }
    };
    match (member.kind, value) {
        (Kind::Float, ShaderValue::Float(v)) => put(buffer, member.offset, v.to_le_bytes()),
        (Kind::Int, ShaderValue::Int(v)) => put(buffer, member.offset, v.to_le_bytes()),
        (Kind::Int, ShaderValue::Float(v)) => put(buffer, member.offset, (*v as i32).to_le_bytes()),
        (Kind::Float, ShaderValue::Int(v)) => put(buffer, member.offset, (*v as f32).to_le_bytes()),
        (Kind::Vector(n), ShaderValue::Vec2(v)) => floats(buffer, member.offset, v, n),
        (Kind::Vector(n), ShaderValue::Vec3(v)) => floats(buffer, member.offset, v, n),
        (Kind::Vector(n), ShaderValue::Vec4(v)) => floats(buffer, member.offset, v, n),
        (Kind::Array { width, len, stride }, ShaderValue::Vec3Array(items)) => {
            for (i, v) in items.iter().take(len).enumerate() {
                floats(buffer, member.offset + i * stride, v, width);
            }
        }
        (Kind::Array { width, len, stride }, ShaderValue::Vec4Array(items)) => {
            for (i, v) in items.iter().take(len).enumerate() {
                floats(buffer, member.offset + i * stride, v, width);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn offsets_follow_the_metal_struct_after_scalar_arrays() {
        let glsl = r#"#version 450
layout(set = 0, binding = 2) uniform Globals {
    vec3 iResolution;
    float iTime;
    float iChannelTime[4];
    vec4 iCurrentCursor;
    int iFocus;
};
layout(location = 0) out vec4 color;
void main() { color = iCurrentCursor * float(iFocus) + iChannelTime[1] + iTime + iResolution.x; }
"#;
        let module = naga::front::glsl::Frontend::default()
            .parse(&naga::front::glsl::Options::from(naga::ShaderStage::Fragment), glsl)
            .unwrap();
        let (_, block) = module.global_variables.iter().find(|(_, g)| g.space == naga::AddressSpace::Uniform).unwrap();
        let (members, size) = super::layout(&module, block.ty);
        // Metal: float3 + float at 0..16, float[4] packed at 16..32, then float4, int.
        assert_eq!(members["iChannelTime"].offset, 16);
        assert_eq!(members["iCurrentCursor"].offset, 32);
        assert_eq!(members["iFocus"].offset, 48);
        assert!(size >= 52);
    }
}
