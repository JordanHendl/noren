use crate::{furikake_state::FurikakeState, rdb::ShaderModule};
use dashi::{BindGroupLayout, BindTableLayout, GraphicsPipeline, GraphicsPipelineLayout, Handle};

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
    pub bind_group_layouts: [Option<Handle<BindGroupLayout>>; 4],
    pub bind_table_layouts: [Option<Handle<BindTableLayout>>; 4],
    pub pipeline_layout: Option<Handle<GraphicsPipelineLayout>>,
    pub pipeline: Option<Handle<GraphicsPipeline>>,
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
