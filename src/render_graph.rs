use std::collections::{BTreeMap, HashMap};

use dashi::{
    BindGroupLayout, BindGroupVariable, BindTableLayout, Context, GraphicsPipeline,
    GraphicsPipelineDetails, GraphicsPipelineLayout, GraphicsPipelineLayoutInfo, Handle,
    PipelineShaderInfo, ShaderPrimitiveType, ShaderType, VertexDescriptionInfo, VertexEntryInfo,
    VertexRate, cfg,
};

use crate::{
    DB, furikake_state::validate_furikake_state, meta::GraphicsShader, meta::ShaderStage,
    parsing::GraphicsShaderLayout, rdb::primitives::Vertex, utils::NorenError,
};

pub struct PipelineFactory<'a> {
    ctx: &'a mut Context,
    render_passes: &'a mut crate::rdb::render_pass::RenderPassDB,
    shaders: &'a mut crate::rdb::shader::ShaderDB,
}

#[derive(Default)]
pub struct RenderGraph {
    pub render_passes: HashMap<String, Handle<dashi::RenderPass>>,
    pub pipelines: HashMap<String, PipelineBinding>,
}

pub struct RenderGraphRequest {
    pub shaders: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct PipelineBinding {
    pub pipeline: Handle<GraphicsPipeline>,
    pub pipeline_layout: Handle<GraphicsPipelineLayout>,
    pub furikake_state: crate::FurikakeState,
    pub bind_group_layouts: [Option<Handle<BindGroupLayout>>; 4],
    pub bind_table_layouts: [Option<Handle<BindTableLayout>>; 4],
}

impl<'a> PipelineFactory<'a> {
    /// Creates a factory that can build pipelines using the provided context and databases.
    pub fn new(
        ctx: &'a mut Context,
        shaders: &'a mut crate::rdb::shader::ShaderDB,
        render_passes: &'a mut crate::rdb::render_pass::RenderPassDB,
    ) -> Self {
        Self {
            ctx,
            render_passes,
            shaders,
        }
    }

    /// Builds a graphics pipeline for the specified shader and render pass layout.
    pub fn make_pipeline(
        &mut self,
        shader_key: &str,
        shader_layout: &GraphicsShaderLayout,
        render_pass_key: &str,
    ) -> Result<(Handle<dashi::RenderPass>, PipelineBinding), NorenError> {
        let shader = DB::load_graphics_shader(self.shaders, shader_key, shader_layout)?
            .ok_or_else(NorenError::LookupFailure)?;

        validate_furikake_state(&shader, shader.furikake_state)?;

        let (bg_layouts, bt_layouts) = self.create_layout_handles(&shader)?;
        let shader_infos = Self::collect_shader_infos(&shader)?;

        let mut vertex_entries = Self::vertex_entries_from_bento(&shader);
        if vertex_entries.is_empty() {
            vertex_entries = Self::default_vertex_entries();
        }

        let vertex_info = VertexDescriptionInfo {
            entries: &vertex_entries,
            stride: std::mem::size_of::<Vertex>(),
            rate: VertexRate::Vertex,
        };

        let layout_info = GraphicsPipelineLayoutInfo {
            debug_name: &shader.name,
            vertex_info,
            bg_layouts,
            bt_layouts,
            shaders: &shader_infos,
            details: GraphicsPipelineDetails::default(),
        };

        let pipeline_layout = self
            .ctx
            .make_graphics_pipeline_layout(&layout_info)
            .map_err(|_| NorenError::UploadFailure())?;

        let pipeline_info = self.render_passes.pipeline_info(
            render_pass_key,
            shader_layout.subpass,
            pipeline_layout,
            &shader.name,
            self.ctx,
        )?;

        let pipeline = self
            .ctx
            .make_graphics_pipeline(&pipeline_info)
            .map_err(|_| NorenError::UploadFailure())?;

        Ok((
            pipeline_info.render_pass,
            PipelineBinding {
                pipeline,
                pipeline_layout,
                furikake_state: shader.furikake_state,
                bind_group_layouts: layout_info.bg_layouts,
                bind_table_layouts: layout_info.bt_layouts,
            },
        ))
    }

    fn create_layout_handles(
        &mut self,
        shader: &GraphicsShader,
    ) -> Result<
        (
            [Option<Handle<BindGroupLayout>>; 4],
            [Option<Handle<BindTableLayout>>; 4],
        ),
        NorenError,
    > {
        let layout_cfgs = Self::shader_bind_group_layouts(shader);

        let mut bg_handles: [Option<Handle<BindGroupLayout>>; 4] = Default::default();
        for (index, cfg_opt) in layout_cfgs.iter().enumerate() {
            if index >= bg_handles.len() {
                break;
            }

            if let Some(cfg) = cfg_opt {
                let borrowed = cfg.borrow();
                let info = borrowed.info();
                let handle = self
                    .ctx
                    .make_bind_group_layout(&info)
                    .map_err(|_| NorenError::UploadFailure())?;
                bg_handles[index] = Some(handle);
            }
        }

        let bt_handles: [Option<Handle<BindTableLayout>>; 4] = Default::default();

        Ok((bg_handles, bt_handles))
    }

    fn collect_shader_infos(
        shader: &GraphicsShader,
    ) -> Result<Vec<PipelineShaderInfo<'_>>, NorenError> {
        let mut shader_infos: Vec<PipelineShaderInfo<'_>> = Vec::new();
        if let Some(stage) = shader.vertex.as_ref() {
            shader_infos.push(PipelineShaderInfo {
                stage: ShaderType::Vertex,
                spirv: stage.module.words(),
                specialization: &[],
            });
        }

        if let Some(stage) = shader.fragment.as_ref() {
            shader_infos.push(PipelineShaderInfo {
                stage: ShaderType::Fragment,
                spirv: stage.module.words(),
                specialization: &[],
            });
        }

        if shader_infos.is_empty() {
            return Err(NorenError::LookupFailure());
        }

        Ok(shader_infos)
    }

    fn shader_bind_group_layouts(shader: &GraphicsShader) -> [Option<cfg::BindGroupLayoutCfg>; 4] {
        let mut shader_sets: [Option<Vec<cfg::ShaderInfoCfg>>; 4] = Default::default();
        for (stage, stage_type) in Self::shader_stages(shader) {
            let mut grouped: BTreeMap<u32, Vec<BindGroupVariable>> = BTreeMap::new();
            for variable in &stage.module.artifact().variables {
                grouped
                    .entry(variable.set)
                    .or_default()
                    .push(variable.kind.clone());
            }

            for (set, variables) in grouped {
                if let Some(slot) = shader_sets.get_mut(set as usize) {
                    let entries = slot.get_or_insert_with(Vec::new);
                    entries.push(cfg::ShaderInfoCfg {
                        stage: stage_type,
                        variables,
                    });
                }
            }
        }

        let mut layouts: [Option<cfg::BindGroupLayoutCfg>; 4] = Default::default();
        for (index, shaders) in shader_sets.into_iter().enumerate() {
            if let Some(shaders) = shaders {
                layouts[index] = Some(cfg::BindGroupLayoutCfg {
                    debug_name: format!("{}_set{index}", shader.name),
                    shaders,
                });
            }
        }

        layouts
    }

    fn shader_stages(shader: &GraphicsShader) -> Vec<(&ShaderStage, ShaderType)> {
        let mut stages = Vec::new();
        if let Some(stage) = shader.vertex.as_ref() {
            stages.push((stage, ShaderType::Vertex));
        }
        if let Some(stage) = shader.fragment.as_ref() {
            stages.push((stage, ShaderType::Fragment));
        }
        stages
    }

    fn default_vertex_entries() -> Vec<VertexEntryInfo> {
        vec![
            VertexEntryInfo {
                format: ShaderPrimitiveType::Vec3,
                location: 0,
                offset: 0,
            },
            VertexEntryInfo {
                format: ShaderPrimitiveType::Vec3,
                location: 1,
                offset: 12,
            },
            VertexEntryInfo {
                format: ShaderPrimitiveType::Vec4,
                location: 2,
                offset: 24,
            },
            VertexEntryInfo {
                format: ShaderPrimitiveType::Vec2,
                location: 3,
                offset: 40,
            },
            VertexEntryInfo {
                format: ShaderPrimitiveType::Vec4,
                location: 4,
                offset: 48,
            },
        ]
    }

    fn vertex_entries_from_bento(shader: &GraphicsShader) -> Vec<VertexEntryInfo> {
        let templates: BTreeMap<u32, VertexEntryInfo> = Self::default_vertex_entries()
            .into_iter()
            .map(|entry| (entry.location as u32, entry))
            .collect();

        let Some(vertex_stage) = shader.vertex.as_ref() else {
            return Vec::new();
        };

        let mut entries = Vec::new();
        for input in &vertex_stage.module.artifact().metadata.inputs {
            if let Some(location) = input.location {
                if let Some(template) = templates.get(&location) {
                    entries.push(template.clone());
                }
            }
        }

        entries
    }
}
