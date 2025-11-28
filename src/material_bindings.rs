use std::collections::HashSet;

use crate::meta::model::GraphicsShader;
use dashi::{BindGroupVariable, BindGroupVariableType};

/// Indicates where a texture binding originates from inside the shader layout.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TextureBindingKind {
    /// A traditional descriptor set slot.
    BindGroup { group: usize, binding: u32 },
    /// A bindless table slot.
    BindTable { table: usize, binding: u32 },
}

/// Description of a single texture slot that a material must provide.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextureBindingSlot {
    pub kind: TextureBindingKind,
    pub element: u32,
    pub required: bool,
}

/// Enumerates all texture slots required by the provided shader metadata.
pub fn texture_binding_slots_from_shader(shader: &GraphicsShader) -> Vec<TextureBindingSlot> {
    let mut bindings: HashSet<TextureBindingSlot> = HashSet::new();

    for (set, variable) in shader_variables(shader) {
        if !matches!(
            variable.var_type,
            BindGroupVariableType::SampledImage | BindGroupVariableType::StorageImage
        ) {
            continue;
        }

        let binding = TextureBindingKind::BindGroup {
            group: set,
            binding: variable.binding,
        };

        for element in 0..variable.count.max(1) {
            bindings.insert(TextureBindingSlot {
                kind: binding.clone(),
                element,
                required: true,
            });
        }
    }

    bindings.into_iter().collect()
}

fn shader_variables(shader: &GraphicsShader) -> Vec<(usize, BindGroupVariable)> {
    let mut variables = Vec::new();
    for stage in [
        shader.vertex.as_ref(),
        shader.fragment.as_ref(),
        shader.geometry.as_ref(),
        shader.tessellation_control.as_ref(),
        shader.tessellation_evaluation.as_ref(),
    ] {
        if let Some(stage) = stage {
            for variable in &stage.module.artifact().variables {
                variables.push((variable.set as usize, variable.kind.clone()));
            }
        }
    }
    variables
}
