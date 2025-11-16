use std::collections::HashMap;

use dashi::{
    BindGroupLayout, BindTableLayout, Context, GraphicsPipeline, GraphicsPipelineDetails,
    GraphicsPipelineLayout, GraphicsPipelineLayoutInfo, Handle, PipelineShaderInfo,
    ShaderPrimitiveType, ShaderType, VertexDescriptionInfo, VertexEntryInfo, VertexRate,
};

use crate::{
    DB, datatypes::primitives::Vertex, meta::GraphicsShader, parsing::GraphicsShaderLayout,
    utils::NorenError,
};

pub struct PipelineFactory<'a> {
    ctx: &'a mut Context,
    render_passes: &'a mut crate::datatypes::render_pass::RenderPassDB,
    shaders: &'a mut crate::datatypes::shader::ShaderDB,
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
    pub bind_group_layouts: [Option<Handle<BindGroupLayout>>; 4],
    pub bind_table_layouts: [Option<Handle<BindTableLayout>>; 4],
}

impl<'a> PipelineFactory<'a> {
    pub fn new(
        ctx: &'a mut Context,
        shaders: &'a mut crate::datatypes::shader::ShaderDB,
        render_passes: &'a mut crate::datatypes::render_pass::RenderPassDB,
    ) -> Self {
        Self {
            ctx,
            render_passes,
            shaders,
        }
    }

    pub fn make_pipeline(
        &mut self,
        shader_key: &str,
        shader_layout: &GraphicsShaderLayout,
        render_pass_key: &str,
    ) -> Result<(Handle<dashi::RenderPass>, PipelineBinding), NorenError> {
        let shader = DB::load_graphics_shader(self.shaders, shader_key, shader_layout)?
            .ok_or_else(NorenError::LookupFailure)?;

        let (bg_layouts, bt_layouts) = self.create_layout_handles(shader_layout)?;
        let shader_infos = Self::collect_shader_infos(&shader)?;

        const VERTEX_ENTRIES: [VertexEntryInfo; 5] = [
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
        ];

        let vertex_info = VertexDescriptionInfo {
            entries: &VERTEX_ENTRIES,
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
                bind_group_layouts: layout_info.bg_layouts,
                bind_table_layouts: layout_info.bt_layouts,
            },
        ))
    }

    fn create_layout_handles(
        &mut self,
        layout: &GraphicsShaderLayout,
    ) -> Result<
        (
            [Option<Handle<BindGroupLayout>>; 4],
            [Option<Handle<BindTableLayout>>; 4],
        ),
        NorenError,
    > {
        let mut bg_handles: [Option<Handle<BindGroupLayout>>; 4] = Default::default();
        for (index, cfg_opt) in layout.bind_group_layouts.iter().enumerate() {
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

        let mut bt_handles: [Option<Handle<BindTableLayout>>; 4] = Default::default();
        for (index, cfg_opt) in layout.bind_table_layouts.iter().enumerate() {
            if index >= bt_handles.len() {
                break;
            }

            if let Some(cfg) = cfg_opt {
                let borrowed = cfg.borrow();
                let info = borrowed.info();
                let handle = self
                    .ctx
                    .make_bind_table_layout(&info)
                    .map_err(|_| NorenError::UploadFailure())?;
                bt_handles[index] = Some(handle);
            }
        }

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
}
