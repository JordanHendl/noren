use std::collections::HashMap;

use dashi::{
    AttachmentDescription, Context, GraphicsPipelineInfo, GraphicsPipelineLayout, Handle,
    RenderPass, SubpassDependency, Viewport, builders::RenderPassBuilder,
};

use crate::{
    parsing::{RenderPassLayout, RenderSubpassLayout},
    utils::NorenError,
};

/// Stores the data needed to construct render passes and caches any created GPU
/// handles.
#[derive(Default)]
pub struct RenderPassDB {
    passes: HashMap<String, RenderPassEntry>,
}

impl RenderPassDB {
    /// Builds a registry from serialized layouts.
    pub fn new(layouts: HashMap<String, RenderPassLayout>) -> Self {
        let passes = layouts
            .into_iter()
            .map(|(key, layout)| {
                let recipe = RenderPassRecipe::from_layout(&key, layout);
                (
                    key,
                    RenderPassEntry {
                        recipe,
                        handle: None,
                    },
                )
            })
            .collect();

        Self { passes }
    }

    /// Fetches the render pass handle for the provided key, lazily creating it
    /// in the supplied context when necessary.
    pub fn fetch(
        &mut self,
        key: &str,
        ctx: &mut Context,
    ) -> Result<Handle<RenderPass>, NorenError> {
        let entry = self
            .passes
            .get_mut(key)
            .ok_or_else(NorenError::LookupFailure)?;

        if let Some(handle) = entry.handle {
            return Ok(handle);
        }

        let handle = entry.recipe.build(ctx)?;
        entry.handle = Some(handle);
        Ok(handle)
    }

    /// Returns pipeline creation info for the requested subpass, creating the render pass if needed.
    pub fn pipeline_info<'a>(
        &'a mut self,
        key: &str,
        subpass: u8,
        layout: Handle<GraphicsPipelineLayout>,
        debug_name: &'a str,
        ctx: &mut Context,
    ) -> Result<GraphicsPipelineInfo<'a>, NorenError> {
        let entry = self
            .passes
            .get_mut(key)
            .ok_or_else(NorenError::LookupFailure)?;

        if subpass as usize >= entry.recipe.subpass_count {
            return Err(NorenError::InvalidRenderPass(format!(
                "subpass {} is out of range for '{}'",
                subpass, key
            )));
        }

        let render_pass = match entry.handle {
            Some(handle) => handle,
            None => {
                let handle = entry.recipe.build(ctx)?;
                entry.handle = Some(handle);
                handle
            }
        };

        Ok(GraphicsPipelineInfo {
            debug_name,
            layout,
            render_pass,
            subpass_id: subpass,
        })
    }
}

struct RenderPassEntry {
    recipe: RenderPassRecipe,
    handle: Option<Handle<RenderPass>>,
}

struct RenderPassRecipe {
    debug_name: String,
    viewport: Viewport,
    subpasses: Vec<RenderPassSubpassRecipe>,
    subpass_count: usize,
}

impl RenderPassRecipe {
    fn from_layout(key: &str, layout: RenderPassLayout) -> Self {
        let debug_name = layout.debug_name.unwrap_or_else(|| key.to_string());
        let subpass_count = layout.subpasses.len();
        let subpasses = layout
            .subpasses
            .into_iter()
            .map(RenderPassSubpassRecipe::from_layout)
            .collect();

        Self {
            debug_name,
            viewport: layout.viewport,
            subpasses,
            subpass_count,
        }
    }

    fn build(&self, ctx: &mut Context) -> Result<Handle<RenderPass>, NorenError> {
        let builder = RenderPassBuilder::new(&self.debug_name, self.viewport);
        let builder = self.subpasses.iter().fold(builder, |builder, subpass| {
            builder.add_subpass(
                &subpass.color_attachments,
                subpass.depth_stencil_attachment.as_ref(),
                &subpass.subpass_dependencies,
            )
        });

        builder.build(ctx).map_err(|_| NorenError::UploadFailure())
    }
}

struct RenderPassSubpassRecipe {
    color_attachments: Vec<AttachmentDescription>,
    depth_stencil_attachment: Option<AttachmentDescription>,
    subpass_dependencies: Vec<SubpassDependency>,
}

impl RenderPassSubpassRecipe {
    fn from_layout(layout: RenderSubpassLayout) -> Self {
        Self {
            color_attachments: layout.color_attachments,
            depth_stencil_attachment: layout.depth_stencil_attachment,
            subpass_dependencies: layout.subpass_dependencies,
        }
    }
}
