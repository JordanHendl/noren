use crate::{furikake_state, furikake_state::FurikakeState, rdb::ShaderModule, utils::NorenError};
use furikake::{BindlessState, DefaultState, GPUState, recipe::RecipeBook};

#[derive(Clone, Debug)]
pub struct BindLayouts {
    pub bg_layouts: [Option<dashi::Handle<dashi::BindGroupLayout>>; 4],
    pub bt_layouts: [Option<dashi::Handle<dashi::BindTableLayout>>; 4],
}

impl Default for BindLayouts {
    fn default() -> Self {
        Self {
            bg_layouts: [None, None, None, None],
            bt_layouts: [None, None, None, None],
        }
    }
}

#[derive(Clone, Debug)]
pub struct GraphicsPipelineInputs {
    pub debug_name: String,
    pub shader: GraphicsShader,
    pub layouts: BindLayouts,
    pub color_formats: Vec<dashi::Format>,
    pub depth_format: Option<dashi::Format>,
    pub subpass_samples: dashi::SubpassSampleInfo,
}

#[derive(Clone, Debug)]
pub struct ComputePipelineInputs {
    pub debug_name: String,
    pub stage: ShaderStage,
    pub layouts: BindLayouts,
}

#[derive(Clone, Debug)]
pub struct ShaderStage {
    pub entry: String,
    pub module: ShaderModule,
}

impl ShaderStage {
    /// Constructs a shader stage from an entry point and compiled module.
    pub fn new(entry: String, module: ShaderModule) -> Self {
        Self { entry, module }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GraphicsShader {
    pub name: String,
    pub vertex: Option<ShaderStage>,
    pub fragment: Option<ShaderStage>,
    pub geometry: Option<ShaderStage>,
    pub tessellation_control: Option<ShaderStage>,
    pub tessellation_evaluation: Option<ShaderStage>,
    pub furikake_state: FurikakeState,
}

impl GraphicsShader {
    /// Creates an empty graphics shader container with the provided display name.
    pub fn new(name: String) -> Self {
        Self {
            name,
            furikake_state: FurikakeState::None,
            ..Default::default()
        }
    }
}

/// Converts a fully loaded graphics shader into pipeline layout inputs.
pub fn graphics_pipeline_inputs(
    ctx: &mut dashi::Context,
    shader_key: &str,
    layout: &crate::parsing::GraphicsShaderLayout,
    shader: GraphicsShader,
) -> Result<GraphicsPipelineInputs, NorenError> {
    if shader.vertex.is_none() {
        return Err(NorenError::InvalidShaderState(format!(
            "graphics shader '{shader_key}' is missing a vertex stage"
        )));
    }

    if layout.color_formats.is_empty() && layout.depth_format.is_none() {
        return Err(NorenError::InvalidShaderState(format!(
            "graphics shader '{shader_key}' does not specify any color or depth formats"
        )));
    }

    let mut stages: Vec<&ShaderStage> = Vec::new();

    if let Some(stage) = shader.vertex.as_ref() {
        ensure_stage_type(stage, dashi::ShaderType::Vertex, shader_key)?;
        stages.push(stage);
    }

    if let Some(stage) = shader.fragment.as_ref() {
        ensure_stage_type(stage, dashi::ShaderType::Fragment, shader_key)?;
        stages.push(stage);
    }

    furikake_state::validate_shader_stages(&stages, shader.furikake_state)?;

    let layouts = furikake_layouts(ctx, shader_key, shader.furikake_state, &stages)?;

    let subpass_samples = dashi::SubpassSampleInfo {
        color_samples: vec![dashi::SampleCount::S1; layout.color_formats.len()],
        depth_sample: layout.depth_format.map(|_| dashi::SampleCount::S1),
    };

    Ok(GraphicsPipelineInputs {
        debug_name: layout
            .name
            .clone()
            .unwrap_or_else(|| shader_key.to_string()),
        shader,
        layouts,
        color_formats: layout.color_formats.clone(),
        depth_format: layout.depth_format,
        subpass_samples,
    })
}

/// Converts a compute shader layout and module into pipeline layout inputs.
pub fn compute_pipeline_inputs(
    ctx: &mut dashi::Context,
    shader_key: &str,
    layout: &crate::parsing::ComputeShaderLayout,
    entry: ShaderStage,
) -> Result<ComputePipelineInputs, NorenError> {
    ensure_stage_type(&entry, dashi::ShaderType::Compute, shader_key)?;

    furikake_state::validate_shader_stages(&[&entry], layout.furikake_state)?;

    let layouts = furikake_layouts(ctx, shader_key, layout.furikake_state, &[&entry])?;

    Ok(ComputePipelineInputs {
        debug_name: layout
            .name
            .clone()
            .unwrap_or_else(|| shader_key.to_string()),
        stage: entry,
        layouts,
    })
}

fn ensure_stage_type(
    stage: &ShaderStage,
    expected: dashi::ShaderType,
    shader_key: &str,
) -> Result<(), NorenError> {
    let artifact = stage.module.artifact();
    if artifact.stage != expected {
        return Err(NorenError::InvalidShaderState(format!(
            "shader module '{}' for '{shader_key}' expected {:?} but found {:?}",
            stage.entry, expected, artifact.stage
        )));
    }

    Ok(())
}

fn furikake_layouts(
    ctx: &mut dashi::Context,
    shader_key: &str,
    state: FurikakeState,
    stages: &[&ShaderStage],
) -> Result<BindLayouts, NorenError> {
    let mut layouts = BindLayouts::default();

    let mut artifacts: Vec<bento::CompilationResult> = stages
        .iter()
        .map(|stage| stage.module.artifact().clone())
        .collect();

    if artifacts.is_empty() || matches!(state, FurikakeState::None) {
        return Ok(layouts);
    }

    let bt_recipes = match state {
        FurikakeState::Default => {
            let fk_state = DefaultState::new(ctx);
            recipe_layouts(ctx, shader_key, &fk_state, &mut artifacts)?
        }
        FurikakeState::Bindless => {
            let fk_state = BindlessState::new(ctx);
            recipe_layouts(ctx, shader_key, &fk_state, &mut artifacts)?
        }
        FurikakeState::None => unreachable!(),
    };

    for recipe in bt_recipes {
        let set = recipe
            .bindings
            .first()
            .map(|b| b.var.set)
            .unwrap_or_default();
        let Some(slot) = layouts.bt_layouts.get_mut(set as usize) else {
            return Err(NorenError::InvalidShaderState(format!(
                "shader '{shader_key}' uses bind table set {set} which exceeds the supported limit"
            )));
        };

        if slot.replace(recipe.layout).is_some() {
            return Err(NorenError::InvalidShaderState(format!(
                "shader '{shader_key}' declares multiple bind table layouts for set {set}"
            )));
        }
    }

    Ok(layouts)
}

fn recipe_layouts<T: GPUState>(
    ctx: &mut dashi::Context,
    shader_key: &str,
    state: &T,
    artifacts: &mut [bento::CompilationResult],
) -> Result<Vec<furikake::recipe::BindTableRecipe>, NorenError> {
    let book = RecipeBook::new(ctx, state, artifacts).map_err(|err| {
        NorenError::InvalidShaderState(format!(
            "furikake validation failed for shader '{shader_key}': {err}"
        ))
    })?;

    Ok(book.recipes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPIRV_MAGIC_WORD: u32 = 0x0723_0203;

    fn mock_stage(stage: dashi::ShaderType) -> ShaderStage {
        let module = ShaderModule::from_compilation(bento::CompilationResult {
            name: Some(format!("mock-{stage:?}")),
            file: None,
            lang: bento::ShaderLang::Glsl,
            stage,
            variables: Vec::new(),
            spirv: vec![SPIRV_MAGIC_WORD, stage as u32],
            metadata: Default::default(),
        });

        ShaderStage::new("main".to_string(), module)
    }

    #[test]
    fn graphics_pipeline_requires_vertex_stage() {
        let mut ctx = dashi::Context::headless(&Default::default()).expect("headless context");
        let layout = crate::parsing::GraphicsShaderLayout {
            name: Some("Pipeline Test".into()),
            fragment: Some("frag".into()),
            color_formats: vec![dashi::Format::RGBA8],
            ..Default::default()
        };

        let shader = GraphicsShader {
            fragment: Some(mock_stage(dashi::ShaderType::Fragment)),
            ..GraphicsShader::new("pipeline".into())
        };

        let result = graphics_pipeline_inputs(&mut ctx, "pipeline", &layout, shader);

        assert!(
            matches!(result, Err(NorenError::InvalidShaderState(msg)) if msg.contains("missing a vertex stage"))
        );
    }

    #[test]
    fn graphics_pipeline_requires_formats() {
        let mut ctx = dashi::Context::headless(&Default::default()).expect("headless context");
        let layout = crate::parsing::GraphicsShaderLayout {
            vertex: Some("vert".into()),
            ..Default::default()
        };

        let shader = GraphicsShader {
            vertex: Some(mock_stage(dashi::ShaderType::Vertex)),
            ..GraphicsShader::new("formats".into())
        };

        let result = graphics_pipeline_inputs(&mut ctx, "formats", &layout, shader);

        assert!(
            matches!(result, Err(NorenError::InvalidShaderState(msg)) if msg.contains("does not specify any color or depth formats"))
        );
    }

    #[test]
    fn graphics_pipeline_validates_stage_type() {
        let mut ctx = dashi::Context::headless(&Default::default()).expect("headless context");
        let layout = crate::parsing::GraphicsShaderLayout {
            name: None,
            vertex: Some("vert".into()),
            color_formats: vec![dashi::Format::RGBA8],
            ..Default::default()
        };

        let shader = GraphicsShader {
            vertex: Some(mock_stage(dashi::ShaderType::Compute)),
            ..GraphicsShader::new("stage-check".into())
        };

        let result = graphics_pipeline_inputs(&mut ctx, "stage-check", &layout, shader);

        assert!(
            matches!(result, Err(NorenError::InvalidShaderState(msg)) if msg.contains("expected Vertex"))
        );
    }

    #[test]
    fn graphics_pipeline_populates_subpass_samples() {
        let mut ctx = dashi::Context::headless(&Default::default()).expect("headless context");
        let layout = crate::parsing::GraphicsShaderLayout {
            name: Some("Named Layout".into()),
            vertex: Some("vert".into()),
            fragment: Some("frag".into()),
            color_formats: vec![dashi::Format::RGBA8, dashi::Format::BGRA8Unorm],
            depth_format: Some(dashi::Format::D24S8),
            ..Default::default()
        };

        let shader = GraphicsShader {
            name: "Named Layout".into(),
            vertex: Some(mock_stage(dashi::ShaderType::Vertex)),
            fragment: Some(mock_stage(dashi::ShaderType::Fragment)),
            ..Default::default()
        };

        let inputs = graphics_pipeline_inputs(&mut ctx, "named", &layout, shader)
            .expect("valid pipeline inputs");

        assert_eq!(inputs.debug_name, "Named Layout");
        assert_eq!(inputs.subpass_samples.color_samples.len(), 2);
        assert!(inputs.subpass_samples.depth_sample.is_some());
    }
}
