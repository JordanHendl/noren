use std::collections::HashSet;

use furikake::{BindlessState, DefaultState, GPUState};
use serde::{Deserialize, Serialize};

use crate::{meta::GraphicsShader, utils::NorenError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FurikakeState {
    #[default]
    None,
    Default,
    Bindless,
}

pub fn validate_furikake_state(
    shader: &GraphicsShader,
    state: FurikakeState,
) -> Result<(), NorenError> {
    match state {
        FurikakeState::None => Ok(()),
        FurikakeState::Default => validate_reserved_bindings::<DefaultState>(shader, state),
        FurikakeState::Bindless => validate_reserved_bindings::<BindlessState>(shader, state),
    }
}

fn validate_reserved_bindings<T: GPUState>(
    shader: &GraphicsShader,
    state: FurikakeState,
) -> Result<(), NorenError> {
    let reserved = T::reserved_metadata();
    let reserved_names: HashSet<&str> = reserved.iter().map(|meta| meta.name).collect();

    let mut variables: Vec<&bento::ShaderVariable> = Vec::new();
    let mut saw_stage = false;

    for stage in shader_stages(shader) {
        saw_stage = true;
        let artifact = stage.module.artifact();
        validate_reserved_not_in_metadata(&reserved_names, &artifact.metadata, state)?;
        variables.extend(artifact.variables.iter());
    }

    if !saw_stage {
        return Err(NorenError::InvalidShaderState(
            "no shader stages available for furikake validation".to_string(),
        ));
    }

    for meta in reserved {
        let Some(found) = variables.iter().find(|var| var.name == meta.name) else {
            return Err(NorenError::InvalidShaderState(format!(
                "reserved binding '{}' required for {:?} state is missing",
                meta.name, state
            )));
        };

        if found.kind.var_type != meta.kind {
            return Err(NorenError::InvalidShaderState(format!(
                "reserved binding '{}' expected {:?} but shader declares {:?}",
                meta.name, meta.kind, found.kind.var_type
            )));
        }
    }

    Ok(())
}

fn validate_reserved_not_in_metadata(
    reserved_names: &HashSet<&str>,
    metadata: &bento::ShaderMetadata,
    state: FurikakeState,
) -> Result<(), NorenError> {
    if let Some(name) = metadata
        .inputs
        .iter()
        .chain(metadata.outputs.iter())
        .map(|variable| variable.name.as_str())
        .find(|name| reserved_names.contains(name))
    {
        return Err(NorenError::InvalidShaderState(format!(
            "reserved binding '{}' must not be listed in shader metadata for {:?} state",
            name, state
        )));
    }

    Ok(())
}

fn shader_stages(shader: &GraphicsShader) -> Vec<&crate::meta::ShaderStage> {
    let mut stages = Vec::new();
    if let Some(stage) = shader.vertex.as_ref() {
        stages.push(stage);
    }
    if let Some(stage) = shader.fragment.as_ref() {
        stages.push(stage);
    }
    if let Some(stage) = shader.geometry.as_ref() {
        stages.push(stage);
    }
    if let Some(stage) = shader.tessellation_control.as_ref() {
        stages.push(stage);
    }
    if let Some(stage) = shader.tessellation_evaluation.as_ref() {
        stages.push(stage);
    }
    stages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{meta::ShaderStage, rdb::ShaderModule};
    use bento::{CompilationResult, InterfaceVariable, ShaderLang, ShaderMetadata, ShaderVariable};
    use dashi::{BindGroupVariable, BindGroupVariableType, ShaderType};

    fn reserved_metadata_for(state: FurikakeState) -> &'static [furikake::ReservedMetadata] {
        match state {
            FurikakeState::Default => furikake::DefaultState::reserved_metadata(),
            FurikakeState::Bindless => furikake::BindlessState::reserved_metadata(),
            FurikakeState::None => &[],
        }
    }

    fn shader_with_variables(
        variables: Vec<ShaderVariable>,
        metadata: ShaderMetadata,
    ) -> GraphicsShader {
        let mut shader = GraphicsShader::new("test".to_string());

        let module = ShaderModule::from_compilation(CompilationResult {
            name: None,
            file: None,
            lang: ShaderLang::Glsl,
            stage: ShaderType::Vertex,
            variables,
            metadata,
            spirv: vec![],
        });

        shader.vertex = Some(ShaderStage::new("main".to_string(), module));
        shader
    }

    fn variables_for_reserved(state: FurikakeState) -> Vec<ShaderVariable> {
        reserved_metadata_for(state)
            .iter()
            .enumerate()
            .map(|(idx, meta)| ShaderVariable {
                name: meta.name.to_string(),
                set: 0,
                kind: BindGroupVariable {
                    var_type: meta.kind.clone(),
                    binding: idx as u32,
                    count: 1,
                },
            })
            .collect()
    }

    #[test]
    fn validates_default_and_bindless_reserved_bindings() {
        for state in [FurikakeState::Default, FurikakeState::Bindless] {
            let shader =
                shader_with_variables(variables_for_reserved(state), ShaderMetadata::default());

            assert!(validate_furikake_state(&shader, state).is_ok());
        }
    }

    #[test]
    fn rejects_missing_reserved_binding() {
        for state in [FurikakeState::Default, FurikakeState::Bindless] {
            let shader = shader_with_variables(Vec::new(), ShaderMetadata::default());

            assert!(matches!(
                validate_furikake_state(&shader, state),
                Err(NorenError::InvalidShaderState(_))
            ));
        }
    }

    #[test]
    fn rejects_reserved_metadata_conflicts() {
        for state in [FurikakeState::Default, FurikakeState::Bindless] {
            let mut metadata = ShaderMetadata::default();
            if let Some(meta) = reserved_metadata_for(state).first() {
                metadata.inputs.push(InterfaceVariable {
                    name: meta.name.to_string(),
                    location: Some(0),
                    format: None,
                });
            }

            let shader = shader_with_variables(variables_for_reserved(state), metadata);

            assert!(matches!(
                validate_furikake_state(&shader, state),
                Err(NorenError::InvalidShaderState(_))
            ));
        }
    }

    #[test]
    fn rejects_reserved_binding_type_mismatches() {
        for state in [FurikakeState::Default, FurikakeState::Bindless] {
            let mut variables = variables_for_reserved(state);
            if let Some(first) = variables.first_mut() {
                first.kind.var_type = BindGroupVariableType::Storage;
            }

            let shader = shader_with_variables(variables, ShaderMetadata::default());

            assert!(matches!(
                validate_furikake_state(&shader, state),
                Err(NorenError::InvalidShaderState(_))
            ));
        }
    }
}
