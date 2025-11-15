use std::{
    collections::{HashMap, HashSet},
    f32::consts::PI,
    path::{Path, PathBuf},
};

use eframe::egui::{self, Color32, ColorImage};
use glam::{Mat4, Vec2, Vec3, Vec4};

use crate::{
    datatypes::{imagery::HostImage, shader::ShaderModule},
    material_editor::project::{GraphTexture, MaterialEditorProjectState},
    material_editor_types::{MaterialEditorDatabaseLayout, MaterialEditorMaterial},
    utils::rdbfile::RDBView,
};

const PREVIEW_WIDTH: usize = 320;
const PREVIEW_HEIGHT: usize = 240;

pub struct MaterialPreviewPanel {
    renderer: PreviewRenderer,
    texture: Option<egui::TextureHandle>,
    texture_label: String,
}

impl MaterialPreviewPanel {
    pub fn new(state: &MaterialEditorProjectState) -> Self {
        let mut renderer = PreviewRenderer::new();
        renderer.ensure_project_location(state.root(), &state.layout);
        Self {
            renderer,
            texture: None,
            texture_label: "material_preview".to_string(),
        }
    }

    pub fn sync_with_state(&mut self, state: &MaterialEditorProjectState) {
        self.renderer
            .ensure_project_location(state.root(), &state.layout);
        self.texture = None;
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        state: &MaterialEditorProjectState,
        material_id: &str,
        material: &MaterialEditorMaterial,
    ) {
        self.renderer
            .ensure_project_location(state.root(), &state.layout);

        egui::CollapsingHeader::new("Preview")
            .default_open(true)
            .show(ui, |ui| {
                self.draw_controls(ui);

                let result = self.renderer.render(material_id, material, state);
                self.update_texture(ui, result.image_changed);

                if let Some(texture) = &self.texture {
                    let aspect = PREVIEW_HEIGHT as f32 / PREVIEW_WIDTH as f32;
                    let width = ui.available_width().min(PREVIEW_WIDTH as f32);
                    let height = width * aspect;
                    let image =
                        egui::Image::new(texture).fit_to_exact_size(egui::vec2(width, height));
                    ui.add(image);
                } else {
                    ui.label("Preview unavailable");
                }

                if !result.warnings.is_empty() {
                    ui.separator();
                    for warning in result.warnings {
                        ui.label(
                            egui::RichText::new(warning).color(Color32::from_rgb(235, 168, 75)),
                        );
                    }
                }
            });
    }

    fn draw_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Mesh");
            egui::ComboBox::from_id_source("preview_mesh_selector")
                .selected_text(self.renderer.config.mesh_kind.label())
                .show_ui(ui, |ui| {
                    for kind in PreviewMeshKind::ALL {
                        let selected = self.renderer.config.mesh_kind == kind;
                        if ui.selectable_label(selected, kind.label()).clicked() {
                            self.renderer.config.mesh_kind = kind;
                        }
                    }
                });
        });

        ui.horizontal(|ui| {
            ui.label("Camera");
            ui.add(
                egui::Slider::new(&mut self.renderer.config.camera.azimuth, -180.0..=180.0)
                    .text("Azimuth"),
            );
            ui.add(
                egui::Slider::new(&mut self.renderer.config.camera.elevation, -80.0..=80.0)
                    .text("Elevation"),
            );
            ui.add(
                egui::Slider::new(&mut self.renderer.config.camera.distance, 1.0..=6.0)
                    .text("Distance"),
            );
            if ui.button("Reset").clicked() {
                self.renderer.config.camera = OrbitCamera::default();
            }
        });

        ui.horizontal(|ui| {
            ui.label("Background");
            ui.color_edit_button_rgb(&mut self.renderer.config.background_rgb);
            ui.checkbox(&mut self.renderer.config.wireframe, "Wireframe");
        });
    }

    fn update_texture(&mut self, ui: &mut egui::Ui, image_changed: bool) {
        let image = self.renderer.image();
        if self.texture.is_none() {
            let handle = ui.ctx().load_texture(
                self.texture_label.clone(),
                image.clone(),
                egui::TextureOptions::LINEAR,
            );
            self.texture = Some(handle);
            return;
        }

        if image_changed {
            if let Some(texture) = &mut self.texture {
                texture.set(image.clone(), egui::TextureOptions::LINEAR);
            }
        }
    }
}

struct PreviewRenderer {
    config: PreviewConfig,
    mesh_cache: PreviewMeshCache,
    assets: PreviewAssetCache,
    image: ColorImage,
}

impl PreviewRenderer {
    fn new() -> Self {
        Self {
            config: PreviewConfig::default(),
            mesh_cache: PreviewMeshCache::new(),
            assets: PreviewAssetCache::default(),
            image: ColorImage::new(
                [PREVIEW_WIDTH, PREVIEW_HEIGHT],
                Color32::from_rgb(30, 30, 30),
            ),
        }
    }

    fn ensure_project_location(&mut self, root: &Path, layout: &MaterialEditorDatabaseLayout) {
        self.assets.ensure_paths(root, layout);
    }

    fn render(
        &mut self,
        material_id: &str,
        material: &MaterialEditorMaterial,
        state: &MaterialEditorProjectState,
    ) -> PreviewResult {
        let mut warnings = Vec::new();

        let base_texture = self.resolve_textures(material, state, &mut warnings);
        let shader_ready = self.ensure_shader_modules(material, state, &mut warnings);

        if base_texture.is_none() {
            warnings.push("No previewable texture bindings; using fallback colors".to_string());
        }
        if !shader_ready {
            warnings.push(format!(
                "Shader for '{}' is incomplete; rendering with default lighting",
                material_id
            ));
        }

        self.draw_scene(base_texture.as_ref());

        PreviewResult {
            warnings,
            image_changed: true,
        }
    }

    fn resolve_textures(
        &mut self,
        material: &MaterialEditorMaterial,
        state: &MaterialEditorProjectState,
        warnings: &mut Vec<String>,
    ) -> Option<PreviewTexture> {
        for texture_id in &material.textures {
            let Some(GraphTexture { resource }) = state.graph.textures.get(texture_id) else {
                warnings.push(format!("Texture '{}' is missing", texture_id));
                continue;
            };
            let image_entry = resource.data.image.clone();
            if image_entry.is_empty() {
                warnings.push(format!(
                    "Texture '{}' does not reference imagery",
                    texture_id
                ));
                continue;
            }
            if let Some(texture) = self.assets.texture(&image_entry) {
                return Some(texture);
            }
            warnings.push(format!(
                "Failed to load imagery '{}' for texture '{}'",
                image_entry, texture_id
            ));
        }
        None
    }

    fn ensure_shader_modules(
        &mut self,
        material: &MaterialEditorMaterial,
        state: &MaterialEditorProjectState,
        warnings: &mut Vec<String>,
    ) -> bool {
        let Some(shader_id) = material.shader.as_ref() else {
            warnings.push("Material does not specify a shader".to_string());
            return false;
        };
        let Some(shader) = state.graph.shaders.get(shader_id) else {
            warnings.push(format!("Shader '{}' is missing", shader_id));
            return false;
        };

        let layout = &shader.resource.data;
        let mut any_stage = false;
        let mut ensure_stage = |stage: Option<&String>, label: &str| {
            if let Some(entry) = stage {
                if entry.is_empty() {
                    warnings.push(format!("Shader stage '{}' is unassigned", label));
                    return;
                }
                if !self.assets.ensure_shader_module(entry) {
                    warnings.push(format!("Failed to load shader module '{}'", entry));
                } else {
                    any_stage = true;
                }
            }
        };

        ensure_stage(layout.vertex.as_ref(), "vertex");
        ensure_stage(layout.fragment.as_ref(), "fragment");
        ensure_stage(layout.geometry.as_ref(), "geometry");
        ensure_stage(layout.tessellation_control.as_ref(), "tessellation control");
        ensure_stage(
            layout.tessellation_evaluation.as_ref(),
            "tessellation evaluation",
        );

        any_stage
    }

    fn draw_scene(&mut self, texture: Option<&PreviewTexture>) {
        let background = Color32::from_rgb(
            (self.config.background_rgb[0].clamp(0.0, 1.0) * 255.0) as u8,
            (self.config.background_rgb[1].clamp(0.0, 1.0) * 255.0) as u8,
            (self.config.background_rgb[2].clamp(0.0, 1.0) * 255.0) as u8,
        );
        self.image = ColorImage::new([PREVIEW_WIDTH, PREVIEW_HEIGHT], background);
        let mut depth = vec![f32::INFINITY; PREVIEW_WIDTH * PREVIEW_HEIGHT];

        let view = self.config.camera.view_matrix();
        let aspect = PREVIEW_WIDTH as f32 / PREVIEW_HEIGHT as f32;
        let proj = Mat4::perspective_rh_gl(45.0_f32.to_radians(), aspect, 0.1, 50.0);
        let vp = proj * view;
        let mesh = self.mesh_cache.mesh(self.config.mesh_kind);
        let light_dir = Vec3::new(0.3, 0.8, 0.6).normalize();
        let fallback_color = [0.7, 0.3, 0.8, 1.0];

        for indices in mesh.indices.chunks(3) {
            if indices.len() < 3 {
                continue;
            }
            let prepared = [
                Self::prepare_vertex(mesh, indices[0], &vp),
                Self::prepare_vertex(mesh, indices[1], &vp),
                Self::prepare_vertex(mesh, indices[2], &vp),
            ];
            if prepared.iter().any(|v| v.is_none()) {
                continue;
            }
            let prepared = [
                prepared[0].as_ref().unwrap(),
                prepared[1].as_ref().unwrap(),
                prepared[2].as_ref().unwrap(),
            ];
            Self::rasterize_triangle(
                &mut self.image.pixels,
                &prepared,
                texture,
                fallback_color,
                &mut depth,
                light_dir,
                self.config.wireframe,
            );
        }
    }

    fn prepare_vertex(mesh: &PreviewMesh, index: u32, vp: &Mat4) -> Option<PreparedVertex> {
        let vertex = mesh.vertices.get(index as usize)?;
        let position = Vec4::new(vertex.position.x, vertex.position.y, vertex.position.z, 1.0);
        let clip = *vp * position;
        if clip.w.abs() < 1e-5 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        let screen = Vec2::new(
            (ndc.x * 0.5 + 0.5) * (PREVIEW_WIDTH as f32 - 1.0),
            (1.0 - (ndc.y * 0.5 + 0.5)) * (PREVIEW_HEIGHT as f32 - 1.0),
        );
        Some(PreparedVertex {
            screen,
            depth: ndc.z,
            normal: vertex.normal,
            uv: vertex.uv,
        })
    }

    fn rasterize_triangle(
        pixels: &mut [Color32],
        vertices: &[&PreparedVertex; 3],
        texture: Option<&PreviewTexture>,
        fallback: [f32; 4],
        depth: &mut [f32],
        light_dir: Vec3,
        wireframe: bool,
    ) {
        let min_x = vertices
            .iter()
            .map(|v| v.screen.x)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as i32;
        let max_x = vertices
            .iter()
            .map(|v| v.screen.x)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min((PREVIEW_WIDTH - 1) as f32) as i32;
        let min_y = vertices
            .iter()
            .map(|v| v.screen.y)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as i32;
        let max_y = vertices
            .iter()
            .map(|v| v.screen.y)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min((PREVIEW_HEIGHT - 1) as f32) as i32;

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                if let Some(bary) = barycentric(
                    p,
                    vertices[0].screen,
                    vertices[1].screen,
                    vertices[2].screen,
                ) {
                    if bary.x < 0.0 || bary.y < 0.0 || bary.z < 0.0 {
                        continue;
                    }
                    let depth_value = bary.x * vertices[0].depth
                        + bary.y * vertices[1].depth
                        + bary.z * vertices[2].depth;
                    let idx = (y as usize) * PREVIEW_WIDTH + x as usize;
                    if depth_value >= depth[idx] {
                        continue;
                    }
                    depth[idx] = depth_value;

                    let uv =
                        bary.x * vertices[0].uv + bary.y * vertices[1].uv + bary.z * vertices[2].uv;
                    let mut color = if let Some(texture) = texture {
                        texture.sample(uv)
                    } else {
                        fallback
                    };

                    let normal = (bary.x * vertices[0].normal
                        + bary.y * vertices[1].normal
                        + bary.z * vertices[2].normal)
                        .normalize();
                    let lighting = (normal.dot(light_dir).max(0.0) * 0.75) + 0.25;
                    color[0] *= lighting;
                    color[1] *= lighting;
                    color[2] *= lighting;

                    if wireframe {
                        let min_component = bary.x.min(bary.y).min(bary.z);
                        if min_component < 0.02 {
                            color = [0.05, 0.05, 0.05, 1.0];
                        }
                    }

                    pixels[idx] = to_color32(color);
                }
            }
        }
    }

    fn image(&self) -> &ColorImage {
        &self.image
    }
}

struct PreviewResult {
    warnings: Vec<String>,
    image_changed: bool,
}

#[derive(Clone, Copy)]
struct PreviewConfig {
    mesh_kind: PreviewMeshKind,
    background_rgb: [f32; 3],
    wireframe: bool,
    camera: OrbitCamera,
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self {
            mesh_kind: PreviewMeshKind::Sphere,
            background_rgb: [0.2, 0.2, 0.25],
            wireframe: false,
            camera: OrbitCamera::default(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PreviewMeshKind {
    Sphere,
    Quad,
}

impl PreviewMeshKind {
    const ALL: [Self; 2] = [Self::Sphere, Self::Quad];

    fn label(self) -> &'static str {
        match self {
            Self::Sphere => "Sphere",
            Self::Quad => "Quad",
        }
    }
}

#[derive(Clone, Copy)]
struct OrbitCamera {
    azimuth: f32,
    elevation: f32,
    distance: f32,
}

impl OrbitCamera {
    fn view_matrix(&self) -> Mat4 {
        let yaw = self.azimuth.to_radians();
        let pitch = self.elevation.to_radians();
        let cos_pitch = pitch.cos();
        let position =
            Vec3::new(yaw.sin() * cos_pitch, pitch.sin(), yaw.cos() * cos_pitch) * self.distance;
        Mat4::look_at_rh(position, Vec3::ZERO, Vec3::Y)
    }
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            azimuth: 45.0,
            elevation: 25.0,
            distance: 2.5,
        }
    }
}

struct PreviewMesh {
    vertices: Vec<PreviewVertex>,
    indices: Vec<u32>,
}

impl PreviewMesh {
    fn sphere() -> Self {
        let stacks = 24;
        let slices = 32;
        let mut vertices = Vec::new();
        for stack in 0..=stacks {
            let v = stack as f32 / stacks as f32;
            let phi = v * PI;
            let y = phi.cos();
            let radius = phi.sin();
            for slice in 0..=slices {
                let u = slice as f32 / slices as f32;
                let theta = u * PI * 2.0;
                let x = radius * theta.cos();
                let z = radius * theta.sin();
                let normal = Vec3::new(x, y, z).normalize();
                vertices.push(PreviewVertex {
                    position: normal,
                    normal,
                    uv: Vec2::new(u, 1.0 - v),
                });
            }
        }

        let mut indices = Vec::new();
        for stack in 0..stacks {
            for slice in 0..slices {
                let first = (stack * (slices + 1) + slice) as u32;
                let second = first + slices as u32 + 1;
                indices.push(first);
                indices.push(second);
                indices.push(first + 1);
                indices.push(second);
                indices.push(second + 1);
                indices.push(first + 1);
            }
        }

        Self { vertices, indices }
    }

    fn quad() -> Self {
        let vertices = vec![
            PreviewVertex {
                position: Vec3::new(-1.0, -1.0, 0.0),
                normal: Vec3::new(0.0, 0.0, 1.0),
                uv: Vec2::new(0.0, 1.0),
            },
            PreviewVertex {
                position: Vec3::new(1.0, -1.0, 0.0),
                normal: Vec3::new(0.0, 0.0, 1.0),
                uv: Vec2::new(1.0, 1.0),
            },
            PreviewVertex {
                position: Vec3::new(1.0, 1.0, 0.0),
                normal: Vec3::new(0.0, 0.0, 1.0),
                uv: Vec2::new(1.0, 0.0),
            },
            PreviewVertex {
                position: Vec3::new(-1.0, 1.0, 0.0),
                normal: Vec3::new(0.0, 0.0, 1.0),
                uv: Vec2::new(0.0, 0.0),
            },
        ];
        let indices = vec![0, 1, 2, 0, 2, 3];
        Self { vertices, indices }
    }
}

struct PreviewVertex {
    position: Vec3,
    normal: Vec3,
    uv: Vec2,
}

struct PreparedVertex {
    screen: Vec2,
    depth: f32,
    normal: Vec3,
    uv: Vec2,
}

struct PreviewMeshCache {
    sphere: PreviewMesh,
    quad: PreviewMesh,
}

impl PreviewMeshCache {
    fn new() -> Self {
        Self {
            sphere: PreviewMesh::sphere(),
            quad: PreviewMesh::quad(),
        }
    }

    fn mesh(&self, kind: PreviewMeshKind) -> &PreviewMesh {
        match kind {
            PreviewMeshKind::Sphere => &self.sphere,
            PreviewMeshKind::Quad => &self.quad,
        }
    }
}

#[derive(Clone)]
struct PreviewTexture {
    width: usize,
    height: usize,
    data: Vec<[f32; 4]>,
}

impl PreviewTexture {
    fn sample(&self, uv: Vec2) -> [f32; 4] {
        if self.width == 0 || self.height == 0 {
            return [1.0, 0.0, 1.0, 1.0];
        }
        let u = uv.x.fract();
        let v = uv.y.fract();
        let u = if u < 0.0 { u + 1.0 } else { u };
        let v = if v < 0.0 { v + 1.0 } else { v };
        let x = ((self.width - 1) as f32 * u).round() as usize;
        let y = ((self.height - 1) as f32 * (1.0 - v)).round() as usize;
        self.data[y * self.width + x]
    }

    fn from_host_image(image: HostImage) -> Option<Self> {
        let width = image.info.dim[0] as usize;
        let height = image.info.dim[1] as usize;
        if width == 0 || height == 0 {
            return None;
        }
        let format = image.info.format;
        let pixels = match format {
            dashi::Format::RGBA8 | dashi::Format::RGBA8Unorm => {
                convert_rgba8(&image.data, width, height, false)
            }
            dashi::Format::BGRA8 | dashi::Format::BGRA8Unorm => {
                convert_rgba8(&image.data, width, height, true)
            }
            dashi::Format::RGBA32F => convert_rgba32f(&image.data, width, height),
            _ => return None,
        };
        Some(Self {
            width,
            height,
            data: pixels,
        })
    }
}

#[derive(Default)]
struct PreviewAssetCache {
    project_root: PathBuf,
    layout: MaterialEditorDatabaseLayout,
    imagery_view: Option<RDBView>,
    shader_view: Option<RDBView>,
    textures: HashMap<String, PreviewTexture>,
    shader_modules: HashSet<String>,
}

impl PreviewAssetCache {
    fn ensure_paths(&mut self, root: &Path, layout: &MaterialEditorDatabaseLayout) {
        if self.project_root != root || self.layout_paths_changed(layout) {
            self.project_root = root.to_path_buf();
            self.layout = layout.clone();
            self.imagery_view = None;
            self.shader_view = None;
            self.textures.clear();
            self.shader_modules.clear();
        }
    }

    fn layout_paths_changed(&self, layout: &MaterialEditorDatabaseLayout) -> bool {
        self.layout.geometry != layout.geometry
            || self.layout.imagery != layout.imagery
            || self.layout.materials != layout.materials
            || self.layout.models != layout.models
            || self.layout.render_passes != layout.render_passes
            || self.layout.shaders != layout.shaders
    }

    fn texture(&mut self, entry: &str) -> Option<PreviewTexture> {
        if let Some(texture) = self.textures.get(entry) {
            return Some(texture.clone());
        }
        let view = self.imagery_view(entry).ok()?;
        let host: HostImage = view.fetch(entry).ok()?;
        let texture = PreviewTexture::from_host_image(host)?;
        self.textures.insert(entry.to_string(), texture.clone());
        Some(texture)
    }

    fn imagery_view(&mut self, entry: &str) -> Result<&mut RDBView, ()> {
        if self.imagery_view.is_none() {
            let path = self.project_root.join(&self.layout.imagery);
            self.imagery_view = RDBView::load(&path).ok();
        }
        self.imagery_view
            .as_mut()
            .ok_or_else(|| log_missing(&self.layout.imagery, entry))
    }

    fn ensure_shader_module(&mut self, entry: &str) -> bool {
        if self.shader_modules.contains(entry) {
            return true;
        }
        let Some(view) = self.shader_view(entry) else {
            return false;
        };
        if view.fetch::<ShaderModule>(entry).is_ok() {
            self.shader_modules.insert(entry.to_string());
            true
        } else {
            false
        }
    }

    fn shader_view(&mut self, entry: &str) -> Option<&mut RDBView> {
        if self.shader_view.is_none() {
            let path = self.project_root.join(&self.layout.shaders);
            self.shader_view = RDBView::load(&path).ok();
        }
        if self.shader_view.is_none() {
            log_missing(&self.layout.shaders, entry);
        }
        self.shader_view.as_mut()
    }
}

fn log_missing(_path: &str, _entry: &str) -> () {
    ()
}

fn convert_rgba8(data: &[u8], width: usize, height: usize, bgra: bool) -> Vec<[f32; 4]> {
    let mut pixels = vec![[0.0; 4]; width * height];
    for (i, chunk) in data.chunks_exact(4).take(width * height).enumerate() {
        let (r, g, b, a) = if bgra {
            (chunk[2], chunk[1], chunk[0], chunk[3])
        } else {
            (chunk[0], chunk[1], chunk[2], chunk[3])
        };
        pixels[i] = [
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            a as f32 / 255.0,
        ];
    }
    pixels
}

fn convert_rgba32f(data: &[u8], width: usize, height: usize) -> Vec<[f32; 4]> {
    let mut pixels = vec![[0.0; 4]; width * height];
    let floats: &[f32] = bytemuck::cast_slice(data);
    for (i, chunk) in floats.chunks_exact(4).take(width * height).enumerate() {
        pixels[i] = [chunk[0], chunk[1], chunk[2], chunk[3]];
    }
    pixels
}

fn barycentric(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> Option<Vec3> {
    let v0 = b - a;
    let v1 = c - a;
    let v2 = p - a;
    let d00 = v0.dot(v0);
    let d01 = v0.dot(v1);
    let d11 = v1.dot(v1);
    let d20 = v2.dot(v0);
    let d21 = v2.dot(v1);
    let denom = d00 * d11 - d01 * d01;
    if denom.abs() < f32::EPSILON {
        return None;
    }
    let v = (d11 * d20 - d01 * d21) / denom;
    let w = (d00 * d21 - d01 * d20) / denom;
    let u = 1.0 - v - w;
    Some(Vec3::new(u, v, w))
}

fn to_color32(color: [f32; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (color[0].clamp(0.0, 1.0) * 255.0) as u8,
        (color[1].clamp(0.0, 1.0) * 255.0) as u8,
        (color[2].clamp(0.0, 1.0) * 255.0) as u8,
        (color[3].clamp(0.0, 1.0) * 255.0) as u8,
    )
}
