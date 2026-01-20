use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

use eframe::egui;
use glam::{Mat4, Vec3, Vec4};
use noren::rdb::terrain::{
    TERRAIN_DIRTY_MUTATION, TerrainChunkDependencyHashes, TerrainChunkState, TerrainDirtyReason,
    TerrainGeneratorDefinition, TerrainMutationLayer, TerrainMutationOp, TerrainMutationOpKind,
    TerrainMutationParams, TerrainProjectSettings, chunk_artifact_entry, chunk_coord_key,
    chunk_state_entry, generator_entry, mutation_layer_entry, mutation_op_entry,
    project_settings_entry,
};
use noren::terrain::{
    TerrainChunkBuildPhase, TerrainChunkBuildRequest, TerrainChunkBuildStatus,
    build_terrain_chunk_with_context, prepare_terrain_build_context, sample_height_with_mutations,
};
use noren::{RDBEntryMeta, RDBFile, RdbErr};

#[derive(Clone, Debug)]
struct ChunkArtifactInfo {
    entry: String,
    region: String,
    coord: Option<(i32, i32)>,
    lod: Option<u8>,
    offset: u64,
    len: u64,
}

#[derive(Clone, Debug)]
struct ProjectState {
    rdb_path: PathBuf,
    key: String,
    settings: TerrainProjectSettings,
    generator: TerrainGeneratorDefinition,
    mutation_layers: Vec<TerrainMutationLayer>,
    chunks: Vec<ChunkArtifactInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Selection {
    None,
    Settings,
    Generator,
    MutationLayer(usize),
    ChunkArtifact(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrushTool {
    SphereAdd,
    SphereSub,
    CapsuleAdd,
    CapsuleSub,
    Smooth,
    MaterialPaint,
}

#[derive(Clone, Debug)]
struct BrushSettings {
    tool: BrushTool,
    radius: f32,
    strength: f32,
    falloff: f32,
    stamp_interval: f64,
    show_grid: bool,
}

#[derive(Clone, Debug, Default)]
struct ViewportState {
    last_stamp_time: Option<f64>,
    last_stamp_pos: Option<[f32; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewportMode {
    Paint,
    Preview,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CameraMode {
    Orbit,
    FreeFly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewScope {
    AllChunks,
    SelectedChunk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LodSelection {
    Auto,
    All,
    Single(u8),
}

#[derive(Clone, Debug)]
struct PreviewCamera {
    yaw: f32,
    pitch: f32,
    distance: f32,
    target: Vec3,
    position: Vec3,
}

impl PreviewCamera {
    fn forward(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        Vec3::new(cos_pitch * cos_yaw, cos_pitch * sin_yaw, sin_pitch).normalize()
    }

    fn orbit_position(&self) -> Vec3 {
        self.target - self.forward() * self.distance
    }
}

#[derive(Clone, Debug)]
struct PreviewSettings {
    mode: ViewportMode,
    camera_mode: CameraMode,
    scope: PreviewScope,
    lod_selection: LodSelection,
    max_chunks: usize,
    max_chunk_distance: f32,
    enable_distance_culling: bool,
    show_wireframe: bool,
    show_bounds: bool,
    show_lod_colors: bool,
    camera_speed: f32,
    orbit_sensitivity: f32,
    fly_sensitivity: f32,
    fov_y_degrees: f32,
    camera: PreviewCamera,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct ChunkMeshKey {
    coords: [i32; 2],
    lod: u8,
}

struct ChunkMeshResource {
    content_hash: u64,
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    indices: Vec<u32>,
    material_ids: Option<Vec<[u32; 4]>>,
    material_weights: Option<Vec<[f32; 4]>>,
    bounds_min: Vec3,
    bounds_max: Vec3,
    entry_offset: u64,
    entry_len: u64,
}

#[derive(Default)]
struct ChunkPreviewCache {
    meshes: HashMap<ChunkMeshKey, ChunkMeshResource>,
}

#[derive(Clone, Debug)]
struct BuildOptions {
    lod: u8,
    build_all_lods: bool,
}

#[derive(Clone, Debug)]
struct BuildJobCurrent {
    chunk: TerrainChunkBuildRequest,
    phase: TerrainChunkBuildPhase,
}

#[derive(Clone, Debug, Default)]
struct BuildJobProgress {
    queued: Vec<TerrainChunkBuildRequest>,
    current: Option<BuildJobCurrent>,
    built: usize,
    skipped: usize,
    cancelled: bool,
    errors: Vec<String>,
}

enum BuildJobEvent {
    Started {
        queued: Vec<TerrainChunkBuildRequest>,
    },
    Phase {
        chunk: TerrainChunkBuildRequest,
        phase: TerrainChunkBuildPhase,
    },
    Built {
        chunk: TerrainChunkBuildRequest,
        artifact: noren::rdb::terrain::TerrainChunkArtifact,
        state: TerrainChunkState,
    },
    Skipped {
        chunk: TerrainChunkBuildRequest,
    },
    Failed {
        chunk: TerrainChunkBuildRequest,
        error: String,
    },
    JobFailed {
        error: String,
    },
    Cancelled,
    Finished,
}

struct BuildJobHandle {
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<BuildJobEvent>,
    handle: JoinHandle<()>,
}

impl ChunkPreviewCache {
    fn upsert(
        &mut self,
        key: ChunkMeshKey,
        artifact: &noren::rdb::terrain::TerrainChunkArtifact,
        entry_offset: u64,
        entry_len: u64,
    ) -> &ChunkMeshResource {
        let needs_update = self
            .meshes
            .get(&key)
            .map(|mesh| {
                mesh.content_hash != artifact.content_hash
                    || mesh.entry_offset != entry_offset
                    || mesh.entry_len != entry_len
            })
            .unwrap_or(true);
        if needs_update {
            let positions = artifact
                .vertices
                .iter()
                .map(|vertex| Vec3::from(vertex.position))
                .collect::<Vec<_>>();
            let normals = artifact
                .vertices
                .iter()
                .map(|vertex| Vec3::from(vertex.normal))
                .collect::<Vec<_>>();
            let material_ids = collect_material_ids(artifact);
            let material_weights = collect_material_weights(artifact);
            let mesh = ChunkMeshResource {
                content_hash: artifact.content_hash,
                positions,
                normals,
                indices: artifact.indices.clone(),
                material_ids,
                material_weights,
                bounds_min: Vec3::from(artifact.bounds_min),
                bounds_max: Vec3::from(artifact.bounds_max),
                entry_offset,
                entry_len,
            };
            self.meshes.insert(key, mesh);
        }
        self.meshes
            .get(&key)
            .expect("mesh should exist after upsert")
    }

    fn ensure_mesh<'a>(
        &'a mut self,
        key: ChunkMeshKey,
        info: &ChunkArtifactInfo,
        rdb: &mut RDBFile,
    ) -> Option<&'a ChunkMeshResource> {
        let s: &mut Self = unsafe { &mut *(self as *mut Self) };
        if let Some(mesh) = self.meshes.get(&key) {
            if mesh.entry_offset == info.offset && mesh.entry_len == info.len {
                return Some(mesh);
            }
        }

        let Ok(artifact) = rdb.fetch::<noren::rdb::terrain::TerrainChunkArtifact>(&info.entry)
        else {
            return None;
        };
        Some(Self::upsert(s, key, &artifact, info.offset, info.len))
    }

    fn retain_keys(&mut self, keys: &HashSet<ChunkMeshKey>) {
        self.meshes.retain(|key, _| keys.contains(key));
    }

    fn get(&self, key: &ChunkMeshKey) -> Option<&ChunkMeshResource> {
        self.meshes.get(key)
    }
}

pub struct TerrainEditorApp {
    rdb_path_input: String,
    project_key_input: String,
    project_keys: Vec<String>,
    rdb: Option<RDBFile>,
    project: Option<ProjectState>,
    selection: Selection,
    active_layer: Option<usize>,
    brush: BrushSettings,
    viewport: ViewportState,
    preview: PreviewSettings,
    preview_cache: ChunkPreviewCache,
    build_options: BuildOptions,
    build_progress: BuildJobProgress,
    build_job: Option<BuildJobHandle>,
    log: Vec<String>,
    validation: Vec<String>,
    last_error: Option<String>,
    show_open_project_dialog: bool,
    show_new_project_dialog: bool,
}

impl Default for TerrainEditorApp {
    fn default() -> Self {
        Self {
            rdb_path_input: String::new(),
            project_key_input: String::new(),
            project_keys: Vec::new(),
            rdb: None,
            project: None,
            selection: Selection::None,
            active_layer: None,
            brush: BrushSettings {
                tool: BrushTool::SphereAdd,
                radius: 8.0,
                strength: 2.0,
                falloff: 0.5,
                stamp_interval: 0.12,
                show_grid: true,
            },
            viewport: ViewportState::default(),
            preview: PreviewSettings {
                mode: ViewportMode::Paint,
                camera_mode: CameraMode::Orbit,
                scope: PreviewScope::AllChunks,
                lod_selection: LodSelection::Auto,
                max_chunks: 256,
                max_chunk_distance: 1400.0,
                enable_distance_culling: true,
                show_wireframe: true,
                show_bounds: false,
                show_lod_colors: true,
                camera_speed: 40.0,
                orbit_sensitivity: 0.01,
                fly_sensitivity: 0.01,
                fov_y_degrees: 50.0,
                camera: PreviewCamera {
                    yaw: 0.8,
                    pitch: 0.4,
                    distance: 140.0,
                    target: Vec3::new(0.0, 0.0, 0.0),
                    position: Vec3::new(0.0, -120.0, 80.0),
                },
            },
            preview_cache: ChunkPreviewCache::default(),
            build_options: BuildOptions {
                lod: 0,
                build_all_lods: true,
            },
            build_progress: BuildJobProgress::default(),
            build_job: None,
            log: Vec::new(),
            validation: Vec::new(),
            last_error: None,
            show_open_project_dialog: false,
            show_new_project_dialog: false,
        }
    }
}

impl TerrainEditorApp {
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New RDB...").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("RDB", &["rdb"])
                        .save_file()
                    {
                        self.rdb_path_input = path.display().to_string();
                        if let Err(err) = self.init_rdb_from_input() {
                            self.set_error(err);
                        }
                    }
                    ui.close_menu();
                }
                if ui.button("Load RDB...").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("RDB", &["rdb"])
                        .pick_file()
                    {
                        self.rdb_path_input = path.display().to_string();
                        if let Err(err) = self.load_rdb_from_input() {
                            self.set_error(err);
                        }
                    }
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Open Project...").clicked() {
                    self.show_open_project_dialog = true;
                    ui.close_menu();
                }
                if ui.button("New Project...").clicked() {
                    self.show_new_project_dialog = true;
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Save Project").clicked() {
                    if let Err(err) = self.save_project() {
                        self.set_error(err);
                    }
                    ui.close_menu();
                }
            });
            ui.menu_button("Edit", |ui| {
                ui.add_enabled(false, egui::Button::new("Undo"));
                ui.add_enabled(false, egui::Button::new("Redo"));
            });
        });

        if let Some(error) = self.last_error.take() {
            ui.colored_label(egui::Color32::RED, error);
        }
    }

    fn project_dialogs(&mut self, ctx: &egui::Context) {
        if self.show_open_project_dialog {
            egui::Window::new("Open Project")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Project key:");
                    if self.project_keys.is_empty() {
                        ui.text_edit_singleline(&mut self.project_key_input);
                    } else {
                        egui::ComboBox::from_id_source("project_key_picker")
                            .selected_text(self.project_key_input.clone())
                            .show_ui(ui, |ui| {
                                for key in &self.project_keys {
                                    if ui
                                        .selectable_label(&self.project_key_input == key, key)
                                        .clicked()
                                    {
                                        self.project_key_input = key.clone();
                                    }
                                }
                            });
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Open").clicked() {
                            if let Err(err) = self.open_project_from_input() {
                                self.set_error(err);
                            }
                            self.show_open_project_dialog = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_open_project_dialog = false;
                        }
                    });
                });
        }

        if self.show_new_project_dialog {
            egui::Window::new("New Project")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Project key:");
                    ui.text_edit_singleline(&mut self.project_key_input);
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked() {
                            if let Err(err) = self.create_project_from_input() {
                                self.set_error(err);
                            }
                            self.show_new_project_dialog = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_new_project_dialog = false;
                        }
                    });
                });
        }
    }

    fn load_rdb_from_input(&mut self) -> Result<(), String> {
        let path = self.rdb_path()?;
        let rdb = RDBFile::load(&path).map_err(|err| format!("Failed to load RDB: {err}"))?;
        self.project_keys = collect_project_keys(&rdb.entries());
        if self.project_key_input.is_empty() {
            if let Some(key) = self.project_keys.first() {
                self.project_key_input = key.clone();
            }
        }
        self.rdb = Some(rdb);
        self.preview_cache.meshes.clear();
        self.log(format!("Loaded RDB: {}", path.display()));
        Ok(())
    }

    fn init_rdb_from_input(&mut self) -> Result<(), String> {
        let path = self.rdb_path()?;
        if path.exists() {
            return Err("RDB already exists at the provided path.".to_string());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("Failed to create RDB folder: {err}"))?;
        }
        let rdb = RDBFile::new();
        rdb.save(&path).map_err(format_rdb_err)?;
        self.rdb = Some(rdb);
        self.project = None;
        self.selection = Selection::None;
        self.active_layer = None;
        self.preview_cache.meshes.clear();
        self.project_keys.clear();
        self.project_key_input.clear();
        self.log(format!(
            "Initialized new RDB: {}. Use New Project to add terrain data.",
            path.display()
        ));
        Ok(())
    }

    fn open_project_from_input(&mut self) -> Result<(), String> {
        let path = self.rdb_path()?;
        if self.rdb.is_none() {
            self.load_rdb_from_input()?;
        }
        let project_key = self.project_key()?;
        let rdb = self
            .rdb
            .as_mut()
            .ok_or_else(|| "RDB not loaded".to_string())?;
        let mut missing_logs = Vec::new();

        if !self.project_keys.is_empty() && !self.project_keys.contains(&project_key) {
            return Err(format!(
                "Project key '{project_key}' not found in RDB. Use New Project to create it."
            ));
        }

        let settings =
            match rdb.fetch::<TerrainProjectSettings>(&project_settings_entry(&project_key)) {
                Ok(value) => value,
                Err(_) => {
                    missing_logs.push("Project settings missing; using defaults.".to_string());
                    TerrainProjectSettings::default()
                }
            };
        let generator = match rdb.fetch::<TerrainGeneratorDefinition>(&generator_entry(
            &project_key,
            settings.active_generator_version,
        )) {
            Ok(value) => value,
            Err(_) => {
                missing_logs.push("Generator definition missing; using defaults.".to_string());
                TerrainGeneratorDefinition::default()
            }
        };
        let mutation_layers = collect_mutation_layers(rdb, &project_key);
        if mutation_layers.is_empty() {
            missing_logs.push("Mutation layers missing; using defaults.".to_string());
        }

        let chunks = collect_chunk_artifacts(&rdb.entries(), &project_key);

        self.project = Some(ProjectState {
            rdb_path: path,
            key: project_key,
            settings,
            generator,
            mutation_layers,
            chunks,
        });
        self.selection = Selection::Settings;
        self.preview_cache.meshes.clear();
        self.active_layer = self
            .project
            .as_ref()
            .and_then(|project| (!project.mutation_layers.is_empty()).then_some(0));
        for message in missing_logs {
            self.log(message);
        }
        self.log("Opened project.");
        Ok(())
    }

    fn create_project_from_input(&mut self) -> Result<(), String> {
        let path = self.rdb_path()?;
        let project_key = self.project_key()?;

        let mut rdb = if path.exists() {
            RDBFile::load(&path).map_err(|err| format!("Failed to open RDB: {err}"))?
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| format!("Failed to create RDB folder: {err}"))?;
            }
            RDBFile::new()
        };

        let mut settings = TerrainProjectSettings {
            name: format!("Terrain Project {project_key}"),
            ..TerrainProjectSettings::default()
        };
        let generator = TerrainGeneratorDefinition::default();
        let mut mutation_layers = vec![TerrainMutationLayer::new("layer-1", "Layer 1", 0)];
        let base_op = TerrainMutationOp {
            op_id: "base-op".to_string(),
            layer_id: "layer-1".to_string(),
            enabled: true,
            order: 0,
            kind: TerrainMutationOpKind::SphereAdd,
            params: TerrainMutationParams::Sphere {
                center: [0.0, 0.0, 0.0],
            },
            radius: 8.0,
            strength: 2.0,
            falloff: 0.5,
            event_id: 1,
            timestamp: current_timestamp(),
            author: None,
        };
        if let Some(layer) = mutation_layers.first_mut() {
            layer.ops.push(base_op.clone());
        }

        settings.active_generator_version = generator.version;
        settings.active_mutation_version = mutation_layers
            .first()
            .map(|layer| layer.version)
            .unwrap_or(1);
        settings.generator_graph_id = generator.graph_id.clone();

        rdb.upsert(&project_settings_entry(&project_key), &settings)
            .map_err(format_rdb_err)?;
        rdb.upsert(
            &generator_entry(&project_key, generator.version),
            &generator,
        )
        .map_err(format_rdb_err)?;
        for layer in &mutation_layers {
            let mut stored_layer = layer.clone();
            stored_layer.ops.clear();
            rdb.upsert(
                &mutation_layer_entry(&project_key, &layer.layer_id, layer.version),
                &stored_layer,
            )
            .map_err(format_rdb_err)?;
        }
        rdb.add(
            &mutation_op_entry(
                &project_key,
                &base_op.layer_id,
                mutation_layers
                    .first()
                    .map(|layer| layer.version)
                    .unwrap_or(1),
                base_op.order,
                base_op.event_id,
            ),
            &base_op,
        )
        .map_err(format_rdb_err)?;
        rdb.save(&path).map_err(format_rdb_err)?;

        self.project_keys = collect_project_keys(&rdb.entries());
        self.project_key_input = project_key.clone();
        self.rdb = Some(rdb);
        self.project = Some(ProjectState {
            rdb_path: path,
            key: project_key,
            settings,
            generator,
            mutation_layers,
            chunks: Vec::new(),
        });
        self.selection = Selection::Settings;
        self.active_layer = Some(0);
        self.preview_cache.meshes.clear();
        self.log("Created new project and saved to RDB.");
        Ok(())
    }

    fn draw_tree(&mut self, ui: &mut egui::Ui) {
        ui.heading("Project Tree");
        let (mut add_layer_clicked, mut new_selection) = (false, None);
        let (project_key, layer_names, chunk_regions): (
            Option<String>,
            Vec<String>,
            Vec<ChunkArtifactInfo>,
        ) = if let Some(project) = &self.project {
            let layer_names = project
                .mutation_layers
                .iter()
                .map(|layer| layer.name.clone())
                .collect::<Vec<_>>();
            let chunk_regions = project.chunks.iter().cloned().collect::<Vec<_>>();
            (Some(project.key.clone()), layer_names, chunk_regions)
        } else {
            (None, Vec::new(), Vec::new())
        };

        if let Some(project_key) = project_key {
            ui.label(format!("Project: {}", project_key));
            ui.add_space(4.0);
            if ui
                .selectable_label(self.selection == Selection::Settings, "Project settings")
                .clicked()
            {
                new_selection = Some(Selection::Settings);
            }
            if ui
                .selectable_label(self.selection == Selection::Generator, "Generator")
                .clicked()
            {
                new_selection = Some(Selection::Generator);
            }

            egui::CollapsingHeader::new("Mutation layers")
                .default_open(true)
                .show(ui, |ui| {
                    for (idx, name) in layer_names.iter().enumerate() {
                        if ui
                            .selectable_label(self.selection == Selection::MutationLayer(idx), name)
                            .clicked()
                        {
                            new_selection = Some(Selection::MutationLayer(idx));
                            self.active_layer = Some(idx);
                        }
                    }
                    if ui.button("Add layer").clicked() {
                        add_layer_clicked = true;
                    }
                });

            egui::CollapsingHeader::new("Chunk build artifacts")
                .default_open(true)
                .show(ui, |ui| {
                    if chunk_regions.is_empty() {
                        ui.label("No chunk artifacts found.");
                    } else {
                        let mut by_region: BTreeMap<&str, Vec<(usize, &ChunkArtifactInfo)>> =
                            BTreeMap::new();
                        for (idx, chunk) in chunk_regions.iter().enumerate() {
                            by_region
                                .entry(chunk.region.as_str())
                                .or_default()
                                .push((idx, chunk));
                        }
                        for (region, entries) in by_region {
                            egui::CollapsingHeader::new(region)
                                .default_open(true)
                                .show(ui, |ui| {
                                    for (idx, chunk) in entries {
                                        let label = match chunk.coord {
                                            Some((x, y)) => format!("({}, {})", x, y),
                                            None => chunk.entry.clone(),
                                        };
                                        if ui
                                            .selectable_label(
                                                self.selection == Selection::ChunkArtifact(idx),
                                                label,
                                            )
                                            .clicked()
                                        {
                                            new_selection = Some(Selection::ChunkArtifact(idx));
                                        }
                                    }
                                });
                        }
                    }
                });
        } else {
            ui.label("Open or create a project to view the tree.");
        }
        if let Some(selection) = new_selection {
            self.selection = selection;
        }
        if add_layer_clicked {
            self.add_layer();
        }
    }

    fn draw_details(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        match self.selection {
            Selection::Settings => {
                if let Some(project) = &mut self.project {
                    ui.heading("Project settings");
                    changed |= ui
                        .text_edit_singleline(&mut project.settings.name)
                        .changed();
                    ui.horizontal(|ui| {
                        ui.label("Seed");
                        changed |= ui
                            .add(egui::DragValue::new(&mut project.settings.seed))
                            .changed();
                    });
                    ui.horizontal(|ui| {
                        ui.label("Tile size");
                        changed |= ui
                            .add(egui::DragValue::new(&mut project.settings.tile_size).speed(0.1))
                            .changed();
                    });
                    ui.horizontal(|ui| {
                        ui.label("Tiles per chunk");
                        changed |= ui
                            .add(egui::DragValue::new(
                                &mut project.settings.tiles_per_chunk[0],
                            ))
                            .changed();
                        changed |= ui
                            .add(egui::DragValue::new(
                                &mut project.settings.tiles_per_chunk[1],
                            ))
                            .changed();
                    });
                }
            }
            Selection::Generator => {
                if let Some(project) = &mut self.project {
                    ui.heading("Generator");
                    changed |= ui
                        .text_edit_singleline(&mut project.generator.algorithm)
                        .changed();
                    ui.horizontal(|ui| {
                        ui.label("Graph ID");
                        changed |= ui
                            .text_edit_singleline(&mut project.generator.graph_id)
                            .changed();
                    });
                    ui.horizontal(|ui| {
                        ui.label("Frequency");
                        changed |= ui
                            .add(egui::DragValue::new(&mut project.generator.frequency).speed(0.01))
                            .changed();
                    });
                    ui.horizontal(|ui| {
                        ui.label("Amplitude");
                        changed |= ui
                            .add(egui::DragValue::new(&mut project.generator.amplitude).speed(0.5))
                            .changed();
                    });
                }
            }
            Selection::MutationLayer(idx) => {
                let mut toggle_events = Vec::new();
                let mut delete_layer = false;
                if let Some(project) = &mut self.project {
                    if let Some(layer) = project.mutation_layers.get_mut(idx) {
                        ui.heading("Mutation layer");
                        changed |= ui.text_edit_singleline(&mut layer.name).changed();
                        ui.separator();
                        ui.label("Operations");
                        for op in &mut layer.ops {
                            ui.horizontal(|ui| {
                                let label = format!("{} ({:?})", op.op_id, op.kind);
                                ui.label(label);
                                if ui.checkbox(&mut op.enabled, "Enabled").changed() {
                                    toggle_events.push(op.clone());
                                }
                            });
                            ui.label(format!(
                                "Order {} | Radius {:.1} | Strength {:.2}",
                                op.order, op.radius, op.strength
                            ));
                            ui.add_space(4.0);
                        }
                        delete_layer = ui.button("Delete layer").clicked();
                        if delete_layer {
                            project.mutation_layers.remove(idx);
                            self.selection = Selection::MutationLayer(idx.saturating_sub(1));
                            if project.mutation_layers.is_empty() {
                                self.active_layer = None;
                            } else {
                                let next_idx =
                                    idx.saturating_sub(1).min(project.mutation_layers.len() - 1);
                                self.active_layer = Some(next_idx);
                            }
                            changed = true;
                        }
                    }
                }
                if !delete_layer {
                    for op in toggle_events {
                        if let Err(err) = self.append_op_toggle(idx, op) {
                            self.set_error(err);
                        }
                    }
                }
            }
            Selection::ChunkArtifact(idx) => {
                if let Some(project) = &self.project {
                    if let Some(chunk) = project.chunks.get(idx) {
                        ui.heading("Chunk build artifact");
                        ui.label(format!("Entry: {}", chunk.entry));
                        ui.label(format!("Region: {}", chunk.region));
                        if let Some((x, y)) = chunk.coord {
                            ui.label(format!("Coord: ({}, {})", x, y));
                        }
                        if let Some(lod) = chunk.lod {
                            ui.label(format!("LOD: {}", lod));
                        }
                    }
                }
            }
            Selection::None => {
                ui.label("Select a node in the tree to view details.");
            }
        }

        if changed {
            if let Err(err) = self.save_project() {
                self.set_error(err);
            }
        }
    }

    fn draw_viewport(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Viewport");
        if self.project.is_none() {
            ui.label("Open a project to begin painting mutations.");
            return;
        }
        self.draw_build_panel(ui);
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label("Mode");
            ui.selectable_value(&mut self.preview.mode, ViewportMode::Paint, "Paint");
            ui.selectable_value(&mut self.preview.mode, ViewportMode::Preview, "Preview");
        });

        match self.preview.mode {
            ViewportMode::Paint => {
                self.draw_brush_controls(ui);
                ui.add_space(8.0);
                self.draw_paint_viewport(ui, ctx);
            }
            ViewportMode::Preview => {
                self.draw_preview_controls(ui);
                ui.add_space(8.0);
                self.draw_preview_viewport(ui, ctx);
            }
        }
    }

    fn draw_paint_viewport(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let available = ui.available_size();
        let viewport_height = available.y.max(220.0);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(available.x, viewport_height),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, egui::Color32::from_gray(20));
        painter.rect_stroke(rect, 0.0, (1.0, egui::Color32::DARK_GRAY));

        if let Some(project) = &self.project {
            let settings = project.settings.clone();
            let layers = project.mutation_layers.clone();
            let project_key = project.key.clone();
            let active_layer = self.active_layer;
            if self.brush.show_grid {
                draw_paint_grid(painter, rect, &settings);
            }
            self.draw_paint_overlay(
                painter,
                rect,
                &settings,
                &layers,
                &project_key,
                active_layer,
            );
        }

        let mut hovered_world = None;
        let settings = self
            .project
            .as_ref()
            .map(|project| project.settings.clone());
        if let (Some(pointer_pos), Some(settings)) = (response.hover_pos(), settings) {
            if let Some(world) = viewport_to_world(&settings, rect, pointer_pos) {
                let height = self.sample_height_for_viewport(world[0], world[1]);
                hovered_world = Some([world[0], world[1], height]);
                let radius_screen = world_radius_to_screen(&settings, rect, self.brush.radius);
                painter.circle_stroke(
                    pointer_pos,
                    radius_screen,
                    (1.0, egui::Color32::from_rgb(120, 200, 255)),
                );
            }
        }

        if let Some(world_pos) = hovered_world {
            ui.label(format!(
                "World: ({:.1}, {:.1}, {:.1})",
                world_pos[0], world_pos[1], world_pos[2]
            ));
            if response.dragged() || response.clicked() {
                let now = ctx.input(|input| input.time);
                if self.should_stamp(now) {
                    if let Err(err) = self.apply_brush_stamp(world_pos) {
                        self.set_error(err);
                    }
                }
            }
        }
    }

    fn draw_paint_overlay(
        &mut self,
        painter: &egui::Painter,
        rect: egui::Rect,
        settings: &TerrainProjectSettings,
        layers: &[TerrainMutationLayer],
        project_key: &str,
        active_layer: Option<usize>,
    ) {
        if let Some(rdb) = self.rdb.as_mut() {
            draw_dirty_overlay(painter, rect, settings, rdb, project_key);
        }
        let max_ops = 400usize;
        let layers = layers
            .iter()
            .enumerate()
            .filter(|(idx, _)| active_layer.map_or(true, |active| active == *idx));
        for (idx, layer) in layers {
            let color = paint_layer_color(idx);
            for op in layer.ops.iter().rev().take(max_ops) {
                if !op.enabled {
                    continue;
                }
                match &op.params {
                    TerrainMutationParams::Sphere { center }
                    | TerrainMutationParams::Smooth { center }
                    | TerrainMutationParams::MaterialPaint {
                        center,
                        material_id: _,
                        blend_mode: _,
                    } => {
                        if let Some(pos) = world_to_viewport(settings, rect, [center[0], center[1]])
                        {
                            let radius = world_radius_to_screen(settings, rect, op.radius);
                            painter.circle_stroke(pos, radius, (1.0, color));
                            painter.circle_filled(pos, 2.5, color);
                        }
                    }
                    TerrainMutationParams::Capsule { start, end } => {
                        let start_pos = world_to_viewport(settings, rect, [start[0], start[1]]);
                        let end_pos = world_to_viewport(settings, rect, [end[0], end[1]]);
                        if let (Some(start_pos), Some(end_pos)) = (start_pos, end_pos) {
                            painter.line_segment([start_pos, end_pos], (1.0, color));
                            let radius = world_radius_to_screen(settings, rect, op.radius);
                            painter.circle_stroke(start_pos, radius, (1.0, color));
                            painter.circle_stroke(end_pos, radius, (1.0, color));
                        }
                    }
                }
            }
        }
    }

    fn draw_brush_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Tool");
            egui::ComboBox::from_id_source("brush_tool")
                .selected_text(format!("{:?}", self.brush.tool))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.brush.tool, BrushTool::SphereAdd, "Sphere Add");
                    ui.selectable_value(&mut self.brush.tool, BrushTool::SphereSub, "Sphere Sub");
                    ui.selectable_value(&mut self.brush.tool, BrushTool::CapsuleAdd, "Capsule Add");
                    ui.selectable_value(&mut self.brush.tool, BrushTool::CapsuleSub, "Capsule Sub");
                    ui.selectable_value(&mut self.brush.tool, BrushTool::Smooth, "Smooth");
                    ui.selectable_value(
                        &mut self.brush.tool,
                        BrushTool::MaterialPaint,
                        "Material Paint",
                    );
                });
        });
        ui.horizontal(|ui| {
            ui.label("Radius");
            ui.add(egui::DragValue::new(&mut self.brush.radius).speed(0.5));
            ui.label("Strength");
            ui.add(egui::DragValue::new(&mut self.brush.strength).speed(0.1));
        });
        ui.horizontal(|ui| {
            ui.label("Falloff");
            ui.add(egui::DragValue::new(&mut self.brush.falloff).speed(0.05));
            ui.label("Stamp interval");
            ui.add(egui::DragValue::new(&mut self.brush.stamp_interval).speed(0.01));
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.brush.show_grid, "Show grid");
        });

        if let Some(project) = &self.project {
            if !project.mutation_layers.is_empty() {
                ui.horizontal(|ui| {
                    ui.label("Layer");
                    let current = self
                        .active_layer
                        .and_then(|idx| project.mutation_layers.get(idx))
                        .map(|layer| layer.name.clone())
                        .unwrap_or_else(|| "None".to_string());
                    egui::ComboBox::from_id_source("active_layer_picker")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for (idx, layer) in project.mutation_layers.iter().enumerate() {
                                if ui
                                    .selectable_label(self.active_layer == Some(idx), &layer.name)
                                    .clicked()
                                {
                                    self.active_layer = Some(idx);
                                }
                            }
                        });
                });
            }
        }
    }

    fn draw_build_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Build");
        let Some(project) = &self.project else {
            ui.label("Open a project to build chunks.");
            return;
        };
        let max_lod = project.settings.lod_policy.max_lod;
        if self.build_options.lod > max_lod {
            self.build_options.lod = max_lod;
        }

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.build_options.build_all_lods, "All LODs");
            ui.label("LOD");
            ui.add(
                egui::DragValue::new(&mut self.build_options.lod)
                    .speed(1)
                    .clamp_range(0..=max_lod),
            );
        });

        let build_active = self.build_job.is_some();
        let mut trigger_dirty = false;
        let mut trigger_selected = false;
        let mut trigger_cancel = false;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!build_active, egui::Button::new("Build dirty chunks"))
                .clicked()
            {
                trigger_dirty = true;
            }
            if ui
                .add_enabled(!build_active, egui::Button::new("Build selected chunk"))
                .clicked()
            {
                trigger_selected = true;
            }
            if ui
                .add_enabled(build_active, egui::Button::new("Cancel build"))
                .clicked()
            {
                trigger_cancel = true;
            }
        });

        if trigger_cancel {
            if let Some(job) = &self.build_job {
                job.cancel.store(true, Ordering::Relaxed);
            }
        }
        let s: &mut Self = unsafe { &mut *(self as *mut Self) };
        if trigger_dirty {
            let lods = self.selected_build_lods();
            if let Some(project) = self.project.as_ref() {
                let requests = Self::collect_dirty_chunk_requests(s, project, &lods);
                self.start_build_job(requests);
            }
        }
        if trigger_selected {
            if let Some(project) = self.project.as_ref() {
                if let Some(requests) = self.collect_selected_chunk_requests(project) {
                    self.start_build_job(requests);
                } else {
                    self.log("No chunk artifact selected to build.");
                }
            }
        }

        if self.build_progress.queued.is_empty() && !build_active {
            ui.label("No build queued.");
            return;
        }

        ui.separator();
        let queued_count = self.build_progress.queued.len();
        let built_count = self.build_progress.built;
        let skipped_count = self.build_progress.skipped;
        let status = if build_active {
            "Running"
        } else if self.build_progress.cancelled {
            "Cancelled"
        } else {
            "Idle"
        };
        ui.label(format!(
            "Status: {status} | Built: {built_count} | Skipped: {skipped_count} | Queued: {queued_count}",
        ));
        if let Some(current) = &self.build_progress.current {
            ui.label(format!(
                "Current: ({}, {}) LOD {} - {}",
                current.chunk.chunk_coords[0],
                current.chunk.chunk_coords[1],
                current.chunk.lod,
                current.phase.label()
            ));
        }

        if !self.build_progress.queued.is_empty() {
            let preview = self
                .build_progress
                .queued
                .iter()
                .take(6)
                .map(|chunk| {
                    format!(
                        "({}, {}) LOD {}",
                        chunk.chunk_coords[0], chunk.chunk_coords[1], chunk.lod
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            ui.label(format!("Queued chunks: {preview}"));
        }

        if !self.build_progress.errors.is_empty() {
            ui.colored_label(egui::Color32::RED, "Build errors:");
            for error in &self.build_progress.errors {
                ui.colored_label(egui::Color32::RED, error);
            }
        }
    }

    fn draw_preview_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Camera");
            ui.selectable_value(&mut self.preview.camera_mode, CameraMode::Orbit, "Orbit");
            ui.selectable_value(
                &mut self.preview.camera_mode,
                CameraMode::FreeFly,
                "Free-fly",
            );
            if ui.button("Reset").clicked() {
                self.preview.camera = PreviewCamera {
                    yaw: 0.8,
                    pitch: 0.4,
                    distance: 140.0,
                    target: Vec3::new(0.0, 0.0, 0.0),
                    position: Vec3::new(0.0, -120.0, 80.0),
                };
            }
        });

        ui.horizontal(|ui| {
            ui.label("Scope");
            ui.selectable_value(
                &mut self.preview.scope,
                PreviewScope::AllChunks,
                "All chunks",
            );
            ui.selectable_value(
                &mut self.preview.scope,
                PreviewScope::SelectedChunk,
                "Selected chunk",
            );
        });

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.preview.show_wireframe, "Wireframe");
            ui.checkbox(&mut self.preview.show_bounds, "Chunk bounds");
            ui.checkbox(&mut self.preview.show_lod_colors, "LOD debug colors");
        });

        let max_lod = self
            .project
            .as_ref()
            .map(|project| project.settings.lod_policy.max_lod)
            .unwrap_or(0);
        ui.horizontal(|ui| {
            ui.label("LOD view");
            ui.selectable_value(&mut self.preview.lod_selection, LodSelection::Auto, "Auto");
            ui.selectable_value(&mut self.preview.lod_selection, LodSelection::All, "All");
            for lod in 0..=max_lod {
                ui.selectable_value(
                    &mut self.preview.lod_selection,
                    LodSelection::Single(lod),
                    format!("{lod}"),
                );
            }
        });

        ui.horizontal(|ui| {
            ui.label("Preview limit");
            ui.add(
                egui::DragValue::new(&mut self.preview.max_chunks)
                    .clamp_range(1..=4096)
                    .speed(1),
            );
            ui.label("chunks");
        });
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut self.preview.enable_distance_culling,
                "Distance culling",
            );
            ui.add_enabled(
                self.preview.enable_distance_culling,
                egui::DragValue::new(&mut self.preview.max_chunk_distance)
                    .clamp_range(100.0..=10000.0)
                    .speed(25.0)
                    .suffix(" units"),
            );
        });
    }

    fn draw_preview_viewport(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let available = ui.available_size();
        let viewport_height = available.y.max(220.0);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(available.x, viewport_height),
            egui::Sense::click_and_drag(),
        );
        if !ui.is_rect_visible(rect) {
            return;
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_gray(14));
        painter.rect_stroke(rect, 0.0, (1.0, egui::Color32::DARK_GRAY));

        self.update_preview_camera(ctx, &response, rect);

        let (camera_pos, camera_target) = self.preview_camera_pose();
        let chunk_infos = self.preview_chunk_infos(camera_pos);
        if chunk_infos.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No chunk artifacts to preview.",
                egui::FontId::proportional(16.0),
                egui::Color32::LIGHT_GRAY,
            );
            return;
        }
        let aspect = (rect.width() / rect.height().max(1.0)).max(0.1);
        let view = Mat4::look_at_rh(camera_pos, camera_target, Vec3::Z);
        let proj =
            Mat4::perspective_rh(self.preview.fov_y_degrees.to_radians(), aspect, 0.1, 6000.0);
        let view_proj = proj * view;
        let light_dir = Vec3::new(0.4, 0.6, 1.0).normalize();

        let mut valid_keys = HashSet::new();
        let mut mesh_keys = Vec::new();
        if let (Some(rdb), Some(project)) = (self.rdb.as_mut(), self.project.as_ref()) {
            let chunk_size = Vec3::new(
                project.settings.tile_size * project.settings.tiles_per_chunk[0] as f32,
                project.settings.tile_size * project.settings.tiles_per_chunk[1] as f32,
                (project.settings.world_bounds_max[2] - project.settings.world_bounds_min[2])
                    .max(1.0),
            );
            let chunk_radius = chunk_size.length() * 0.5;
            let mut candidate_infos: Vec<(f32, ChunkArtifactInfo)> = Vec::new();
            for info in chunk_infos {
                let mut distance = 0.0;
                if let Some(coord) = info.coord {
                    let center = Vec3::new(
                        coord.0 as f32 * chunk_size.x + chunk_size.x * 0.5,
                        coord.1 as f32 * chunk_size.y + chunk_size.y * 0.5,
                        project.settings.world_bounds_min[2] + chunk_size.z * 0.5,
                    );
                    distance = (center - camera_pos).length();
                    if self.preview.enable_distance_culling
                        && (distance - chunk_radius).max(0.0) > self.preview.max_chunk_distance
                    {
                        continue;
                    }
                }
                candidate_infos.push((distance, info));
            }
            candidate_infos
                .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            for (_, info) in candidate_infos
                .into_iter()
                .take(self.preview.max_chunks.max(1))
            {
                let Some((coords, lod)) = info.coord.zip(info.lod) else {
                    continue;
                };
                let key = ChunkMeshKey {
                    coords: [coords.0, coords.1],
                    lod,
                };
                if self.preview_cache.ensure_mesh(key, &info, rdb).is_some() {
                    valid_keys.insert(key);
                    mesh_keys.push(key);
                }
            }
        }
        self.preview_cache.retain_keys(&valid_keys);

        let meshes_to_draw = mesh_keys
            .iter()
            .filter_map(|key| self.preview_cache.get(key).map(|mesh| (*key, mesh)))
            .collect::<Vec<_>>();

        if meshes_to_draw.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No meshes loaded",
                egui::FontId::proportional(16.0),
                egui::Color32::LIGHT_GRAY,
            );
            return;
        }

        let center = {
            let mut min = Vec3::splat(f32::MAX);
            let mut max = Vec3::splat(f32::MIN);
            for (_, mesh) in &meshes_to_draw {
                min = min.min(mesh.bounds_min);
                max = max.max(mesh.bounds_max);
            }
            (min + max) * 0.5
        };

        #[derive(Clone)]
        struct PreviewTriangle {
            vertices: [egui::epaint::Vertex; 3],
            depth: f32,
        }

        let mut triangles: Vec<PreviewTriangle> = Vec::new();
        let mut wireframe_segments: Vec<(egui::Pos2, egui::Pos2)> = Vec::new();
        let mut bounds_segments: Vec<(egui::Pos2, egui::Pos2)> = Vec::new();

        for (key, mesh) in meshes_to_draw {
            for tri in mesh.indices.chunks(3) {
                if tri.len() < 3 {
                    continue;
                }
                let i0 = tri[0] as usize;
                let i1 = tri[1] as usize;
                let i2 = tri[2] as usize;
                let Some((p0, d0)) = project_point(mesh.positions[i0] - center, view_proj, rect)
                else {
                    continue;
                };
                let Some((p1, d1)) = project_point(mesh.positions[i1] - center, view_proj, rect)
                else {
                    continue;
                };
                let Some((p2, d2)) = project_point(mesh.positions[i2] - center, view_proj, rect)
                else {
                    continue;
                };
                let n0 = mesh.normals.get(i0).copied().unwrap_or(Vec3::Z).normalize();
                let n1 = mesh.normals.get(i1).copied().unwrap_or(Vec3::Z).normalize();
                let n2 = mesh.normals.get(i2).copied().unwrap_or(Vec3::Z).normalize();
                let base0 = if self.preview.show_lod_colors {
                    lod_debug_color(key.lod)
                } else {
                    triplanar_tint(
                        material_color_for_vertex(mesh, i0),
                        mesh.positions[i0],
                        n0,
                    )
                };
                let base1 = if self.preview.show_lod_colors {
                    lod_debug_color(key.lod)
                } else {
                    triplanar_tint(
                        material_color_for_vertex(mesh, i1),
                        mesh.positions[i1],
                        n1,
                    )
                };
                let base2 = if self.preview.show_lod_colors {
                    lod_debug_color(key.lod)
                } else {
                    triplanar_tint(
                        material_color_for_vertex(mesh, i2),
                        mesh.positions[i2],
                        n2,
                    )
                };
                let c0 = shade_color(base0, n0, light_dir);
                let c1 = shade_color(base1, n1, light_dir);
                let c2 = shade_color(base2, n2, light_dir);
                let depth = (d0 + d1 + d2) / 3.0;
                triangles.push(PreviewTriangle {
                    vertices: [
                        egui::epaint::Vertex {
                            pos: p0,
                            uv: egui::Pos2::ZERO,
                            color: c0,
                        },
                        egui::epaint::Vertex {
                            pos: p1,
                            uv: egui::Pos2::ZERO,
                            color: c1,
                        },
                        egui::epaint::Vertex {
                            pos: p2,
                            uv: egui::Pos2::ZERO,
                            color: c2,
                        },
                    ],
                    depth,
                });
                if self.preview.show_wireframe {
                    wireframe_segments.push((p0, p1));
                    wireframe_segments.push((p1, p2));
                    wireframe_segments.push((p2, p0));
                }
            }

            if self.preview.show_bounds {
                bounds_segments.extend(project_bounds(
                    mesh.bounds_min - center,
                    mesh.bounds_max - center,
                    view_proj,
                    rect,
                ));
            }
        }

        triangles.sort_by(|a, b| {
            a.depth
                .partial_cmp(&b.depth)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut mesh = egui::Mesh::default();
        for tri in triangles {
            let base = mesh.vertices.len() as u32;
            mesh.vertices.extend_from_slice(&tri.vertices);
            mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
        painter.add(egui::Shape::mesh(mesh));

        if self.preview.show_wireframe {
            for (start, end) in wireframe_segments {
                painter.line_segment([start, end], (1.0, egui::Color32::from_gray(200)));
            }
        }

        if self.preview.show_bounds {
            for (start, end) in bounds_segments {
                painter.line_segment([start, end], (1.0, egui::Color32::from_rgb(255, 160, 80)));
            }
        }
    }

    fn preview_chunk_infos(&self, camera_pos: Vec3) -> Vec<ChunkArtifactInfo> {
        let Some(project) = &self.project else {
            return Vec::new();
        };
        let mut chunks: Vec<ChunkArtifactInfo> = match self.preview.scope {
            PreviewScope::AllChunks => project.chunks.clone(),
            PreviewScope::SelectedChunk => match self.selection {
                Selection::ChunkArtifact(idx) => project
                    .chunks
                    .get(idx)
                    .cloned()
                    .map(|chunk| vec![chunk])
                    .unwrap_or_default(),
                _ => Vec::new(),
            },
        };

        match self.preview.lod_selection {
            LodSelection::Auto => {
                self.select_lods_for_preview(chunks, &project.settings, camera_pos)
            }
            LodSelection::All => chunks,
            LodSelection::Single(lod) => {
                chunks.retain(|chunk| chunk.lod == Some(lod));
                chunks
            }
        }
    }

    fn lod_for_distance(&self, settings: &TerrainProjectSettings, distance: f32) -> u8 {
        let mut lod = 0_u8;
        for (idx, band) in settings.lod_policy.distance_bands.iter().enumerate() {
            if distance > *band {
                lod = lod.saturating_add(1).max((idx + 1) as u8);
            }
        }
        lod.min(settings.lod_policy.max_lod)
    }

    fn select_lods_for_preview(
        &self,
        chunks: Vec<ChunkArtifactInfo>,
        settings: &TerrainProjectSettings,
        camera_pos: Vec3,
    ) -> Vec<ChunkArtifactInfo> {
        let mut by_coord: HashMap<(i32, i32), Vec<ChunkArtifactInfo>> = HashMap::new();
        let mut fallbacks = Vec::new();
        for chunk in chunks {
            if let Some(coord) = chunk.coord {
                by_coord.entry(coord).or_default().push(chunk);
            } else {
                fallbacks.push(chunk);
            }
        }

        let chunk_size = Vec3::new(
            settings.tile_size * settings.tiles_per_chunk[0] as f32,
            settings.tile_size * settings.tiles_per_chunk[1] as f32,
            (settings.world_bounds_max[2] - settings.world_bounds_min[2]).max(1.0),
        );
        let mut selected = Vec::new();
        for (coord, mut options) in by_coord {
            options.sort_by_key(|info| info.lod.unwrap_or(0));
            let center = Vec3::new(
                coord.0 as f32 * chunk_size.x + chunk_size.x * 0.5,
                coord.1 as f32 * chunk_size.y + chunk_size.y * 0.5,
                settings.world_bounds_min[2] + chunk_size.z * 0.5,
            );
            let distance = (center - camera_pos).length();
            let desired = self.lod_for_distance(settings, distance);
            if let Some(info) = options
                .iter()
                .find(|info| info.lod == Some(desired))
                .cloned()
            {
                selected.push(info);
                continue;
            }
            if let Some(info) = options
                .iter()
                .min_by_key(|info| info.lod.unwrap_or(0).abs_diff(desired))
                .cloned()
            {
                selected.push(info);
            }
        }

        selected.extend(fallbacks);
        selected
    }

    fn preview_camera_pose(&self) -> (Vec3, Vec3) {
        let forward = self.preview.camera.forward();
        match self.preview.camera_mode {
            CameraMode::Orbit => (
                self.preview.camera.orbit_position(),
                self.preview.camera.target,
            ),
            CameraMode::FreeFly => (
                self.preview.camera.position,
                self.preview.camera.position + forward,
            ),
        }
    }

    fn update_preview_camera(
        &mut self,
        ctx: &egui::Context,
        response: &egui::Response,
        rect: egui::Rect,
    ) {
        if !response.hovered() {
            return;
        }
        let drag_delta = response.drag_delta();
        let shift = ctx.input(|input| input.modifiers.shift);
        let scroll_delta = ctx.input(|input| input.raw_scroll_delta.y);

        let sensitivity = match self.preview.camera_mode {
            CameraMode::Orbit => self.preview.orbit_sensitivity,
            CameraMode::FreeFly => self.preview.fly_sensitivity,
        };

        if response.dragged_by(egui::PointerButton::Primary) {
            if self.preview.camera_mode == CameraMode::Orbit && shift {
                let (camera_pos, camera_target) = self.preview_camera_pose();
                let forward = (camera_target - camera_pos).normalize_or_zero();
                let right = forward.cross(Vec3::Z).normalize_or_zero();
                let up = right.cross(forward).normalize_or_zero();
                let scale = self.preview.camera.distance * 0.002;
                let offset = (-right * drag_delta.x + up * drag_delta.y) * scale;
                self.preview.camera.target += offset;
            } else {
                self.preview.camera.yaw -= drag_delta.x * sensitivity;
                self.preview.camera.pitch =
                    (self.preview.camera.pitch - drag_delta.y * sensitivity).clamp(-1.5, 1.5);
            }
        }

        match self.preview.camera_mode {
            CameraMode::Orbit => {
                if scroll_delta.abs() > 0.0 {
                    self.preview.camera.distance =
                        (self.preview.camera.distance - scroll_delta * 0.4).clamp(8.0, 8000.0);
                }
            }
            CameraMode::FreeFly => {
                let dt = ctx.input(|input| input.unstable_dt).max(0.016);
                let speed = if shift {
                    self.preview.camera_speed * 2.5
                } else {
                    self.preview.camera_speed
                };
                let forward = self.preview.camera.forward();
                let right = forward.cross(Vec3::Z).normalize_or_zero();
                let mut velocity = Vec3::ZERO;
                if ctx.input(|input| input.key_down(egui::Key::W)) {
                    velocity += forward;
                }
                if ctx.input(|input| input.key_down(egui::Key::S)) {
                    velocity -= forward;
                }
                if ctx.input(|input| input.key_down(egui::Key::A)) {
                    velocity -= right;
                }
                if ctx.input(|input| input.key_down(egui::Key::D)) {
                    velocity += right;
                }
                if ctx.input(|input| input.key_down(egui::Key::Q)) {
                    velocity -= Vec3::Z;
                }
                if ctx.input(|input| input.key_down(egui::Key::E)) {
                    velocity += Vec3::Z;
                }
                if velocity.length_squared() > 0.0 {
                    self.preview.camera.position += velocity.normalize_or_zero() * speed * dt;
                }
            }
        }

        if response.double_clicked()
            && rect.contains(response.interact_pointer_pos().unwrap_or(rect.center()))
        {
            self.preview.camera.target = Vec3::new(0.0, 0.0, 0.0);
        }
    }

    fn selected_build_lods(&self) -> Vec<u8> {
        let max_lod = self
            .project
            .as_ref()
            .map(|project| project.settings.lod_policy.max_lod)
            .unwrap_or(0);
        if self.build_options.build_all_lods {
            (0..=max_lod).collect()
        } else {
            vec![self.build_options.lod.min(max_lod)]
        }
    }

    fn collect_dirty_chunk_requests(
        &mut self,
        project: &ProjectState,
        lods: &[u8],
    ) -> Vec<TerrainChunkBuildRequest> {
        let Some(rdb) = self.rdb.as_mut() else {
            return Vec::new();
        };
        let prefix = format!("terrain/chunk_state/{}/", project.key);
        let mut requests = Vec::new();
        for entry in rdb.entries() {
            if let Some(coord) = parse_chunk_state_entry(&entry.name, &prefix) {
                if let Ok(state) = rdb.fetch::<TerrainChunkState>(&entry.name) {
                    if state.dirty_flags == 0 {
                        continue;
                    }
                    for lod in lods {
                        requests.push(TerrainChunkBuildRequest {
                            chunk_coords: [coord.0, coord.1],
                            lod: *lod,
                        });
                    }
                }
            }
        }
        requests
    }

    fn collect_selected_chunk_requests(
        &self,
        project: &ProjectState,
    ) -> Option<Vec<TerrainChunkBuildRequest>> {
        let Selection::ChunkArtifact(idx) = self.selection else {
            return None;
        };
        let chunk = project.chunks.get(idx)?;
        let coord = chunk.coord?;
        let lods = if self.build_options.build_all_lods {
            (0..=project.settings.lod_policy.max_lod).collect::<Vec<_>>()
        } else if let Some(lod) = chunk.lod {
            vec![lod]
        } else {
            self.selected_build_lods()
        };
        Some(
            lods.into_iter()
                .map(|lod| TerrainChunkBuildRequest {
                    chunk_coords: [coord.0, coord.1],
                    lod,
                })
                .collect(),
        )
    }

    fn start_build_job(&mut self, requests: Vec<TerrainChunkBuildRequest>) {
        if requests.is_empty() {
            self.log("No chunks queued for build.");
            return;
        }
        if self.build_job.is_some() {
            self.log("Build already running.");
            return;
        }
        let Some(project) = &self.project else {
            return;
        };
        let rdb_path = project.rdb_path.clone();
        let project_key = project.key.clone();
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_thread = Arc::clone(&cancel);
        let build_requests = requests.clone();
        let handle = thread::spawn(move || {
            let _ = sender.send(BuildJobEvent::Started {
                queued: build_requests.clone(),
            });
            let mut rdb = match RDBFile::load(&rdb_path) {
                Ok(rdb) => rdb,
                Err(err) => {
                    let _ = sender.send(BuildJobEvent::JobFailed {
                        error: format!("Failed to load RDB: {err}"),
                    });
                    let _ = sender.send(BuildJobEvent::Finished);
                    return;
                }
            };
            let context = match prepare_terrain_build_context(&mut rdb, &project_key) {
                Ok(context) => context,
                Err(err) => {
                    let _ = sender.send(BuildJobEvent::JobFailed {
                        error: format!("Failed to prepare build context: {err}"),
                    });
                    let _ = sender.send(BuildJobEvent::Finished);
                    return;
                }
            };

            for request in build_requests {
                if cancel_thread.load(Ordering::Relaxed) {
                    let _ = sender.send(BuildJobEvent::Cancelled);
                    break;
                }
                let phase_sender = &sender;
                let outcome = build_terrain_chunk_with_context(
                    &mut rdb,
                    &project_key,
                    &context,
                    request,
                    |phase| {
                        let _ = phase_sender.send(BuildJobEvent::Phase {
                            chunk: request,
                            phase,
                        });
                    },
                    || cancel_thread.load(Ordering::Relaxed),
                );
                let outcome = match outcome {
                    Ok(outcome) => outcome,
                    Err(err) => {
                        let _ = sender.send(BuildJobEvent::Failed {
                            chunk: request,
                            error: format!("Build failed: {err}"),
                        });
                        continue;
                    }
                };
                match outcome.status {
                    TerrainChunkBuildStatus::Skipped => {
                        let _ = sender.send(BuildJobEvent::Skipped { chunk: request });
                    }
                    TerrainChunkBuildStatus::Cancelled => {
                        let _ = sender.send(BuildJobEvent::Cancelled);
                        break;
                    }
                    TerrainChunkBuildStatus::Built => {
                        if cancel_thread.load(Ordering::Relaxed) {
                            let _ = sender.send(BuildJobEvent::Cancelled);
                            break;
                        }
                        let _ = sender.send(BuildJobEvent::Phase {
                            chunk: request,
                            phase: TerrainChunkBuildPhase::Write,
                        });
                        let Some(artifact) = outcome.artifact else {
                            let _ = sender.send(BuildJobEvent::Failed {
                                chunk: request,
                                error: "Missing artifact".to_string(),
                            });
                            continue;
                        };
                        let Some(state) = outcome.state else {
                            let _ = sender.send(BuildJobEvent::Failed {
                                chunk: request,
                                error: "Missing state".to_string(),
                            });
                            continue;
                        };
                        let coord_key =
                            chunk_coord_key(request.chunk_coords[0], request.chunk_coords[1]);
                        let artifact_key = chunk_artifact_entry(
                            &project_key,
                            &coord_key,
                            &format!("lod{}", request.lod),
                        );
                        let state_key = chunk_state_entry(&project_key, &coord_key);
                        if let Err(err) = rdb.upsert(&artifact_key, &artifact) {
                            let _ = sender.send(BuildJobEvent::Failed {
                                chunk: request,
                                error: format!("Failed to update artifact: {err}"),
                            });
                            continue;
                        }
                        if let Err(err) = rdb.upsert(&state_key, &state) {
                            let _ = sender.send(BuildJobEvent::Failed {
                                chunk: request,
                                error: format!("Failed to update chunk state: {err}"),
                            });
                            continue;
                        }
                        if let Err(err) = save_rdb_atomic(&rdb, &rdb_path) {
                            let _ = sender.send(BuildJobEvent::Failed {
                                chunk: request,
                                error: format!("Failed to save RDB: {err}"),
                            });
                            continue;
                        }
                        let _ = sender.send(BuildJobEvent::Built {
                            chunk: request,
                            artifact,
                            state,
                        });
                    }
                }
            }
            let _ = sender.send(BuildJobEvent::Finished);
        });

        self.build_progress = BuildJobProgress::default();
        self.build_progress.queued = requests;
        self.build_job = Some(BuildJobHandle {
            cancel,
            receiver,
            handle,
        });
        self.log("Queued build job.");
    }

    fn poll_build_events(&mut self, ctx: &egui::Context) {
        let mut job_finished = false;
        let events = if let Some(job) = self.build_job.as_ref() {
            let mut events = Vec::new();
            while let Ok(event) = job.receiver.try_recv() {
                events.push(event);
            }
            events
        } else {
            Vec::new()
        };

        for event in events {
            match event {
                BuildJobEvent::Started { queued } => {
                    self.build_progress = BuildJobProgress {
                        queued,
                        ..BuildJobProgress::default()
                    };
                }
                BuildJobEvent::Phase { chunk, phase } => {
                    self.build_progress.current = Some(BuildJobCurrent { chunk, phase });
                }
                BuildJobEvent::Built {
                    chunk,
                    artifact,
                    state,
                } => {
                    self.build_progress.built += 1;
                    self.build_progress.queued.retain(|entry| *entry != chunk);
                    self.build_progress.current = None;
                    if let Some(rdb) = self.rdb.as_mut() {
                        let coord_key =
                            chunk_coord_key(chunk.chunk_coords[0], chunk.chunk_coords[1]);
                        let artifact_key = chunk_artifact_entry(
                            &state.project_key,
                            &coord_key,
                            &format!("lod{}", chunk.lod),
                        );
                        let state_key = chunk_state_entry(&state.project_key, &coord_key);
                        let _ = rdb.upsert(&artifact_key, &artifact);
                        let _ = rdb.upsert(&state_key, &state);
                    }
                    if let (Some(rdb), Some(project)) = (self.rdb.as_ref(), self.project.as_mut()) {
                        let entries = rdb.entries();
                        project.chunks = collect_chunk_artifacts(&entries, &project.key);
                    }
                    let key = ChunkMeshKey {
                        coords: chunk.chunk_coords,
                        lod: chunk.lod,
                    };
                    if self.preview.mode == ViewportMode::Preview {
                        if let Some(project) = self.project.as_ref() {
                            let coord_key =
                                chunk_coord_key(chunk.chunk_coords[0], chunk.chunk_coords[1]);
                            let artifact_key = chunk_artifact_entry(
                                &project.key,
                                &coord_key,
                                &format!("lod{}", chunk.lod),
                            );
                            if let Some(info) = project
                                .chunks
                                .iter()
                                .find(|chunk_info| chunk_info.entry == artifact_key)
                            {
                                self.preview_cache
                                    .upsert(key, &artifact, info.offset, info.len);
                            } else {
                                self.preview_cache.meshes.remove(&key);
                            }
                        }
                    } else {
                        self.preview_cache.meshes.remove(&key);
                    }
                    self.log(format!(
                        "Built chunk ({}, {}) LOD {}",
                        chunk.chunk_coords[0], chunk.chunk_coords[1], chunk.lod
                    ));
                }
                BuildJobEvent::Skipped { chunk } => {
                    self.build_progress.skipped += 1;
                    self.build_progress.queued.retain(|entry| *entry != chunk);
                    self.build_progress.current = None;
                    self.log(format!(
                        "Skipped chunk ({}, {}) LOD {}",
                        chunk.chunk_coords[0], chunk.chunk_coords[1], chunk.lod
                    ));
                }
                BuildJobEvent::Failed { chunk, error } => {
                    self.build_progress.errors.push(format!(
                        "Chunk ({}, {}) LOD {}: {error}",
                        chunk.chunk_coords[0], chunk.chunk_coords[1], chunk.lod
                    ));
                    self.build_progress.queued.retain(|entry| *entry != chunk);
                    self.build_progress.current = None;
                }
                BuildJobEvent::JobFailed { error } => {
                    self.build_progress.errors.push(error.clone());
                    self.log(format!("Build job failed: {error}"));
                }
                BuildJobEvent::Cancelled => {
                    self.build_progress.cancelled = true;
                    self.log("Build job cancelled.");
                }
                BuildJobEvent::Finished => {
                    job_finished = true;
                }
            }
        }

        if let Some(job) = &self.build_job {
            if job.handle.is_finished() || job_finished {
                if let Some(job) = self.build_job.take() {
                    let _ = job.handle.join();
                }
            }
        }

        if self.build_job.is_some() {
            ctx.request_repaint();
        }
    }

    fn should_stamp(&mut self, now: f64) -> bool {
        let allow = self
            .viewport
            .last_stamp_time
            .map(|last| now - last >= self.brush.stamp_interval)
            .unwrap_or(true);
        if allow {
            self.viewport.last_stamp_time = Some(now);
        }
        allow
    }

    fn apply_brush_stamp(&mut self, world_pos: [f32; 3]) -> Result<(), String> {
        let (rdb, project) = match (self.rdb.as_mut(), self.project.as_mut()) {
            (Some(rdb), Some(project)) => (rdb, project),
            _ => return Ok(()),
        };
        let layer_index = self
            .active_layer
            .or_else(|| (!project.mutation_layers.is_empty()).then_some(0))
            .ok_or_else(|| "No mutation layer available".to_string())?;
        let layer_name = project
            .mutation_layers
            .get(layer_index)
            .map(|layer| layer.name.clone())
            .ok_or_else(|| "Invalid mutation layer".to_string())?;
        let layer_id = project
            .mutation_layers
            .get(layer_index)
            .map(|layer| layer.layer_id.clone())
            .ok_or_else(|| "Invalid mutation layer".to_string())?;
        let layer_version = project
            .mutation_layers
            .get(layer_index)
            .map(|layer| layer.version)
            .ok_or_else(|| "Invalid mutation layer".to_string())?;

        let (order, event_id) =
            next_op_order_and_event(rdb, &project.key, &layer_id, layer_version, None);
        let op_id = format!("op-{}", current_timestamp());
        let (kind, params) =
            build_params_for_brush(self.brush.tool, world_pos, self.viewport.last_stamp_pos);
        let op = TerrainMutationOp {
            op_id,
            layer_id: layer_id.clone(),
            enabled: true,
            order,
            kind,
            params,
            radius: self.brush.radius.max(0.0),
            strength: self.brush.strength,
            falloff: self.brush.falloff.clamp(0.0, 1.0),
            event_id,
            timestamp: current_timestamp(),
            author: None,
        };
        rdb.add(
            &mutation_op_entry(
                &project.key,
                &layer_id,
                layer_version,
                op.order,
                op.event_id,
            ),
            &op,
        )
        .map_err(format_rdb_err)?;

        if let Some(layer) = project.mutation_layers.get_mut(layer_index) {
            layer.ops.push(op.clone());
            layer
                .ops
                .sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.op_id.cmp(&b.op_id)));
        }
        self.viewport.last_stamp_pos = Some(world_pos);
        mark_chunks_dirty(rdb, project, &op)?;
        rdb.save(&project.rdb_path).map_err(format_rdb_err)?;
        self.log(format!("Stamped op on layer {}", layer_name));
        Ok(())
    }

    fn append_op_toggle(
        &mut self,
        layer_index: usize,
        op: TerrainMutationOp,
    ) -> Result<(), String> {
        let (rdb, project) = match (self.rdb.as_mut(), self.project.as_mut()) {
            (Some(rdb), Some(project)) => (rdb, project),
            _ => return Ok(()),
        };
        let layer = project
            .mutation_layers
            .get(layer_index)
            .ok_or_else(|| "Invalid mutation layer".to_string())?;
        let (_, next_event) = next_op_order_and_event(
            rdb,
            &project.key,
            &layer.layer_id,
            layer.version,
            Some(&op.op_id),
        );
        let toggled = TerrainMutationOp {
            enabled: op.enabled,
            event_id: next_event,
            timestamp: current_timestamp(),
            ..op
        };
        rdb.add(
            &mutation_op_entry(
                &project.key,
                &layer.layer_id,
                layer.version,
                toggled.order,
                toggled.event_id,
            ),
            &toggled,
        )
        .map_err(format_rdb_err)?;
        if let Some(layer) = project.mutation_layers.get_mut(layer_index) {
            if let Some(existing) = layer
                .ops
                .iter_mut()
                .find(|entry| entry.op_id == toggled.op_id)
            {
                *existing = toggled.clone();
            }
        }
        mark_chunks_dirty(rdb, project, &toggled)?;
        rdb.save(&project.rdb_path).map_err(format_rdb_err)?;
        Ok(())
    }

    fn sample_height_for_viewport(&mut self, world_x: f32, world_y: f32) -> f32 {
        let project = match &self.project {
            Some(project) => project,
            None => return 0.0,
        };
        if let Some(rdb) = self.rdb.as_mut() {
            let chunk = chunk_coords_for_world(&project.settings, world_x, world_y);
            let coord_key = chunk_coord_key(chunk.0, chunk.1);
            let entry = chunk_artifact_entry(&project.key, &coord_key, "lod0");
            if let Ok(artifact) = rdb.fetch::<noren::rdb::terrain::TerrainChunkArtifact>(&entry) {
                if let Some(height) =
                    height_from_artifact(&project.settings, &artifact, world_x, world_y)
                {
                    return height;
                }
            }
        }

        let coarse = project.settings.tile_size.max(0.1) * 4.0;
        let snapped_x = (world_x / coarse).round() * coarse;
        let snapped_y = (world_y / coarse).round() * coarse;
        sample_height_with_mutations(
            &project.settings,
            &project.generator,
            &project.mutation_layers,
            snapped_x,
            snapped_y,
        )
    }

    fn draw_log(&mut self, ui: &mut egui::Ui) {
        ui.heading("Build log");
        egui::ScrollArea::vertical()
            .max_height(120.0)
            .show(ui, |ui| {
                for entry in &self.log {
                    ui.label(entry);
                }
            });
        ui.add_space(4.0);
        ui.heading("Validation messages");
        if self.validation.is_empty() {
            ui.label("No validation issues.");
        } else {
            for issue in &self.validation {
                ui.colored_label(egui::Color32::YELLOW, issue);
            }
        }
    }

    fn save_project(&mut self) -> Result<(), String> {
        let (rdb, project) = match (self.rdb.as_mut(), self.project.as_mut()) {
            (Some(rdb), Some(project)) => (rdb, project),
            _ => return Ok(()),
        };

        project.settings.active_generator_version = project.generator.version;
        project.settings.generator_graph_id = project.generator.graph_id.clone();
        if let Some(max_version) = project
            .mutation_layers
            .iter()
            .map(|layer| layer.version)
            .max()
        {
            project.settings.active_mutation_version = max_version;
        }

        rdb.upsert(&project_settings_entry(&project.key), &project.settings)
            .map_err(format_rdb_err)?;
        rdb.upsert(
            &generator_entry(&project.key, project.generator.version),
            &project.generator,
        )
        .map_err(format_rdb_err)?;
        for layer in &project.mutation_layers {
            let mut stored_layer = layer.clone();
            stored_layer.ops.clear();
            rdb.upsert(
                &mutation_layer_entry(&project.key, &layer.layer_id, layer.version),
                &stored_layer,
            )
            .map_err(format_rdb_err)?;
        }
        rdb.save(&project.rdb_path).map_err(format_rdb_err)?;

        project.chunks = collect_chunk_artifacts(&rdb.entries(), &project.key);
        self.log("Saved project data to RDB.");
        Ok(())
    }

    fn add_layer(&mut self) {
        if let Some(project) = &mut self.project {
            let idx = project.mutation_layers.len() + 1;
            let layer_id = format!("layer-{idx}");
            project.mutation_layers.push(TerrainMutationLayer::new(
                layer_id,
                format!("Layer {idx}"),
                idx as u32 - 1,
            ));
            self.selection = Selection::MutationLayer(project.mutation_layers.len() - 1);
            self.active_layer = Some(project.mutation_layers.len() - 1);
            if let Err(err) = self.save_project() {
                self.set_error(err);
            }
        }
    }

    fn log(&mut self, message: impl Into<String>) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| format!("{:>6}", duration.as_secs()))
            .unwrap_or_else(|_| "------".to_string());
        self.log.push(format!("[{timestamp}] {}", message.into()));
    }

    fn set_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.last_error = Some(message.clone());
        self.log(format!("Error: {message}"));
    }

    fn rdb_path(&self) -> Result<PathBuf, String> {
        let trimmed = self.rdb_path_input.trim();
        if trimmed.is_empty() {
            return Err("RDB path is required".to_string());
        }
        Ok(PathBuf::from(trimmed))
    }

    fn project_key(&self) -> Result<String, String> {
        let trimmed = self.project_key_input.trim();
        if trimmed.is_empty() {
            return Err("Project key is required".to_string());
        }
        Ok(trimmed.to_string())
    }

    fn refresh_validation(&mut self) {
        self.validation.clear();
        if let Some(project) = &self.project {
            if project.settings.tile_size <= 0.0 {
                self.validation
                    .push("Tile size must be greater than zero.".to_string());
            }
            if project.settings.tiles_per_chunk[0] == 0 || project.settings.tiles_per_chunk[1] == 0
            {
                self.validation
                    .push("Tiles per chunk must be at least 1x1.".to_string());
            }
            if project.generator.frequency <= 0.0 {
                self.validation
                    .push("Generator frequency must be greater than zero.".to_string());
            }
            if project.mutation_layers.is_empty() {
                self.validation
                    .push("At least one mutation layer is recommended.".to_string());
            }
        }
    }
}

impl eframe::App for TerrainEditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_build_events(ctx);
        self.refresh_validation();

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            self.toolbar(ui);
        });

        egui::SidePanel::left("project_tree").show(ctx, |ui| {
            self.draw_tree(ui);
        });

        egui::SidePanel::right("details_panel").show(ctx, |ui| {
            self.draw_details(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.draw_viewport(ui, ctx);
        });

        egui::TopBottomPanel::bottom("log_panel").show(ctx, |ui| {
            self.draw_log(ui);
        });

        self.project_dialogs(ctx);
    }
}

fn collect_project_keys(entries: &[RDBEntryMeta]) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for entry in entries {
        let parts: Vec<&str> = entry.name.split('/').collect();
        if parts.len() >= 3 && parts[0] == "terrain" && parts[1] == "project" {
            keys.insert(parts[2].to_string());
        }
    }
    keys.into_iter().collect()
}

fn collect_chunk_artifacts(entries: &[RDBEntryMeta], project_key: &str) -> Vec<ChunkArtifactInfo> {
    let mut artifacts = Vec::new();
    let prefix = format!("terrain/chunk_artifact/{project_key}/");
    for entry in entries {
        if entry.name.starts_with(&prefix) {
            if let Some(info) = parse_project_chunk(entry, project_key) {
                artifacts.push(info);
            }
        } else if entry.name.starts_with("terrain/chunk_") {
            if let Some(info) = parse_legacy_chunk(entry) {
                artifacts.push(info);
            }
        }
    }
    artifacts
}

fn collect_dirty_chunks(rdb: &mut RDBFile, project_key: &str) -> Vec<[i32; 2]> {
    let prefix = format!("terrain/chunk_state/{project_key}/");
    let mut chunks = Vec::new();
    for entry in rdb.entries() {
        if let Some(coord) = parse_chunk_state_entry(&entry.name, &prefix) {
            if let Ok(state) = rdb.fetch::<TerrainChunkState>(&entry.name) {
                if state.dirty_flags != 0 {
                    chunks.push([coord.0, coord.1]);
                }
            }
        }
    }
    chunks
}

fn draw_dirty_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    settings: &TerrainProjectSettings,
    rdb: &mut RDBFile,
    project_key: &str,
) {
    let dirty_chunks = collect_dirty_chunks(rdb, project_key);
    if dirty_chunks.is_empty() {
        return;
    }
    let chunk_size_x = settings.tiles_per_chunk[0] as f32 * settings.tile_size;
    let chunk_size_y = settings.tiles_per_chunk[1] as f32 * settings.tile_size;
    let tint = egui::Color32::from_rgba_premultiplied(255, 120, 120, 40);
    let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 120, 120));

    for coords in dirty_chunks {
        let min_x = settings.world_bounds_min[0] + coords[0] as f32 * chunk_size_x;
        let min_y = settings.world_bounds_min[1] + coords[1] as f32 * chunk_size_y;
        let max_x = min_x + chunk_size_x;
        let max_y = min_y + chunk_size_y;

        let min_pos = world_to_viewport(settings, rect, [min_x, max_y]);
        let max_pos = world_to_viewport(settings, rect, [max_x, min_y]);
        let (Some(min_pos), Some(max_pos)) = (min_pos, max_pos) else {
            continue;
        };
        let chunk_rect = egui::Rect::from_min_max(min_pos, max_pos);
        painter.rect_filled(chunk_rect, 0.0, tint);
        painter.rect_stroke(chunk_rect, 0.0, stroke);
    }
}

fn draw_paint_grid(painter: &egui::Painter, rect: egui::Rect, settings: &TerrainProjectSettings) {
    let min = settings.world_bounds_min;
    let max = settings.world_bounds_max;
    let range_x = max[0] - min[0];
    let range_y = max[1] - min[1];
    if range_x <= 0.0 || range_y <= 0.0 {
        return;
    }

    let pixel_per_world = rect.width() / range_x.max(f32::EPSILON);
    let mut spacing = settings.tile_size.max(0.1);
    let min_pixels = 18.0;
    while spacing * pixel_per_world < min_pixels {
        spacing *= 2.0;
        if spacing > range_x.max(range_y) {
            break;
        }
    }

    let grid_color = egui::Color32::from_rgba_premultiplied(90, 90, 90, 50);
    let chunk_color = egui::Color32::from_rgba_premultiplied(140, 140, 140, 90);
    let grid_stroke = egui::Stroke::new(1.0, grid_color);
    let chunk_stroke = egui::Stroke::new(1.0, chunk_color);
    let max_lines = 2048;

    let mut x = (min[0] / spacing).floor() * spacing;
    let mut drawn = 0;
    while x <= max[0] && drawn < max_lines {
        let start = world_to_viewport(settings, rect, [x, min[1]]);
        let end = world_to_viewport(settings, rect, [x, max[1]]);
        if let (Some(start), Some(end)) = (start, end) {
            painter.line_segment([start, end], grid_stroke);
        }
        x += spacing;
        drawn += 1;
    }

    let mut y = (min[1] / spacing).floor() * spacing;
    drawn = 0;
    while y <= max[1] && drawn < max_lines {
        let start = world_to_viewport(settings, rect, [min[0], y]);
        let end = world_to_viewport(settings, rect, [max[0], y]);
        if let (Some(start), Some(end)) = (start, end) {
            painter.line_segment([start, end], grid_stroke);
        }
        y += spacing;
        drawn += 1;
    }

    let chunk_size_x = settings.tiles_per_chunk[0] as f32 * settings.tile_size;
    let chunk_size_y = settings.tiles_per_chunk[1] as f32 * settings.tile_size;
    if chunk_size_x > 0.0 && chunk_size_y > 0.0 {
        let mut x = (min[0] / chunk_size_x).floor() * chunk_size_x;
        drawn = 0;
        while x <= max[0] && drawn < max_lines {
            let start = world_to_viewport(settings, rect, [x, min[1]]);
            let end = world_to_viewport(settings, rect, [x, max[1]]);
            if let (Some(start), Some(end)) = (start, end) {
                painter.line_segment([start, end], chunk_stroke);
            }
            x += chunk_size_x;
            drawn += 1;
        }

        let mut y = (min[1] / chunk_size_y).floor() * chunk_size_y;
        drawn = 0;
        while y <= max[1] && drawn < max_lines {
            let start = world_to_viewport(settings, rect, [min[0], y]);
            let end = world_to_viewport(settings, rect, [max[0], y]);
            if let (Some(start), Some(end)) = (start, end) {
                painter.line_segment([start, end], chunk_stroke);
            }
            y += chunk_size_y;
            drawn += 1;
        }
    }
}

fn parse_project_chunk(entry: &RDBEntryMeta, project_key: &str) -> Option<ChunkArtifactInfo> {
    let prefix = format!("terrain/chunk_artifact/{project_key}/");
    let remainder = entry.name.strip_prefix(&prefix)?;
    let mut parts = remainder.split('/');
    let coord_part = parts.next()?;
    let lod_part = parts.next()?;
    Some(ChunkArtifactInfo {
        entry: entry.name.clone(),
        region: "default".to_string(),
        coord: parse_coord(coord_part),
        lod: parse_lod(lod_part),
        offset: entry.offset,
        len: entry.len,
    })
}

fn parse_legacy_chunk(entry: &RDBEntryMeta) -> Option<ChunkArtifactInfo> {
    let remainder = entry.name.strip_prefix("terrain/chunk_")?;
    Some(ChunkArtifactInfo {
        entry: entry.name.clone(),
        region: "legacy".to_string(),
        coord: parse_coord(remainder),
        lod: None,
        offset: entry.offset,
        len: entry.len,
    })
}

fn parse_coord(value: &str) -> Option<(i32, i32)> {
    let mut parts = value.split('_');
    let x = parts.next()?.parse().ok()?;
    let y = parts.next()?.parse().ok()?;
    Some((x, y))
}

fn parse_lod(value: &str) -> Option<u8> {
    value.strip_prefix("lod")?.parse().ok()
}

fn parse_chunk_state_entry(name: &str, prefix: &str) -> Option<(i32, i32)> {
    let remainder = name.strip_prefix(prefix)?;
    parse_coord(remainder)
}

fn format_rdb_err(err: RdbErr) -> String {
    format!("RDB error: {err}")
}

fn save_rdb_atomic(rdb: &RDBFile, path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "RDB path has no parent directory".to_string())?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Invalid RDB filename".to_string())?;
    let temp_path = parent.join(format!("{file_name}.tmp"));
    rdb.save(&temp_path).map_err(format_rdb_err)?;
    if path.exists() {
        std::fs::remove_file(path).map_err(|err| format!("Failed to remove old RDB: {err}"))?;
    }
    std::fs::rename(&temp_path, path)
        .map_err(|err| format!("Failed to replace RDB file: {err}"))?;
    Ok(())
}

fn collect_mutation_layers(rdb: &mut RDBFile, project_key: &str) -> Vec<TerrainMutationLayer> {
    let prefix = format!("terrain/mutation_layer/{project_key}/");
    let mut layer_versions = BTreeMap::new();
    for entry in rdb.entries() {
        if let Some((layer_id, version)) = parse_mutation_layer_entry(&entry.name, &prefix) {
            let current = layer_versions.entry(layer_id).or_insert(version);
            if version > *current {
                *current = version;
            }
        }
    }

    let mut layers = Vec::new();
    for (layer_id, version) in layer_versions {
        let entry = mutation_layer_entry(project_key, &layer_id, version);
        if let Ok(layer) = rdb.fetch::<TerrainMutationLayer>(&entry) {
            let mut layer = layer;
            let ops = collect_mutation_ops(rdb, project_key, &layer_id, version);
            if !ops.is_empty() {
                layer.ops = ops;
            }
            layers.push(layer);
        }
    }
    layers.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then_with(|| a.layer_id.cmp(&b.layer_id))
    });
    layers
}

fn parse_mutation_layer_entry(name: &str, prefix: &str) -> Option<(String, u32)> {
    let remainder = name.strip_prefix(prefix)?;
    let mut parts = remainder.split('/');
    let layer_id = parts.next()?.to_string();
    let version_part = parts.next()?;
    let version = version_part.strip_prefix('v')?.parse().ok()?;
    Some((layer_id, version))
}

fn collect_mutation_ops(
    rdb: &mut RDBFile,
    project_key: &str,
    layer_id: &str,
    version: u32,
) -> Vec<TerrainMutationOp> {
    let prefix = format!("terrain/mutation_op/{project_key}/{layer_id}/");
    let mut latest: BTreeMap<String, TerrainMutationOp> = BTreeMap::new();
    for entry in rdb.entries() {
        if let Some((entry_version, order, event_id)) =
            parse_mutation_op_entry(&entry.name, &prefix)
        {
            if entry_version != version {
                continue;
            }
            if let Ok(mut op) = rdb.fetch::<TerrainMutationOp>(&entry.name) {
                op.order = order;
                op.event_id = event_id;
                latest
                    .entry(op.op_id.clone())
                    .and_modify(|current| {
                        if op.event_id > current.event_id {
                            *current = op.clone();
                        }
                    })
                    .or_insert(op);
            }
        }
    }
    let mut ops: Vec<TerrainMutationOp> = latest.into_values().collect();
    ops.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.op_id.cmp(&b.op_id)));
    ops
}

fn parse_mutation_op_entry(name: &str, prefix: &str) -> Option<(u32, u32, u32)> {
    let remainder = name.strip_prefix(prefix)?;
    let mut parts = remainder.split('/');
    let version_part = parts.next()?;
    let order_part = parts.next()?;
    let event_part = parts.next()?;
    let version = version_part.strip_prefix('v')?.parse().ok()?;
    let order = order_part.strip_prefix('o')?.parse().ok()?;
    let event_id = event_part.strip_prefix('e')?.parse().ok()?;
    Some((version, order, event_id))
}

fn next_op_order_and_event(
    rdb: &mut RDBFile,
    project_key: &str,
    layer_id: &str,
    version: u32,
    op_id: Option<&str>,
) -> (u32, u32) {
    let prefix = format!("terrain/mutation_op/{project_key}/{layer_id}/v{version}/");
    let mut max_order = 0;
    let mut max_event = 0;
    for entry in rdb.entries() {
        if let Some(remainder) = entry.name.strip_prefix(&prefix) {
            let mut parts = remainder.split('/');
            if let (Some(order_part), Some(event_part)) = (parts.next(), parts.next()) {
                if let (Some(order), Some(event_id)) = (
                    order_part
                        .strip_prefix('o')
                        .and_then(|v| v.parse::<u32>().ok()),
                    event_part
                        .strip_prefix('e')
                        .and_then(|v| v.parse::<u32>().ok()),
                ) {
                    max_order = max_order.max(order + 1);
                    if let Some(op_id) = op_id {
                        if let Ok(op) = rdb.fetch::<TerrainMutationOp>(&entry.name) {
                            if op.op_id == op_id {
                                max_event = max_event.max(event_id);
                            }
                        }
                    }
                }
            }
        }
    }
    let next_event = if op_id.is_some() { max_event + 1 } else { 1 };
    (max_order, next_event)
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn viewport_to_world(
    settings: &TerrainProjectSettings,
    rect: egui::Rect,
    pointer_pos: egui::Pos2,
) -> Option<[f32; 2]> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let u = ((pointer_pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    let v = ((pointer_pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
    let min = settings.world_bounds_min;
    let max = settings.world_bounds_max;
    let world_x = min[0] + u * (max[0] - min[0]);
    let world_y = max[1] - v * (max[1] - min[1]);
    Some([world_x, world_y])
}

fn world_to_viewport(
    settings: &TerrainProjectSettings,
    rect: egui::Rect,
    world_pos: [f32; 2],
) -> Option<egui::Pos2> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let min = settings.world_bounds_min;
    let max = settings.world_bounds_max;
    let range_x = (max[0] - min[0]).max(f32::EPSILON);
    let range_y = (max[1] - min[1]).max(f32::EPSILON);
    let u = (world_pos[0] - min[0]) / range_x;
    let v = (max[1] - world_pos[1]) / range_y;
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return None;
    }
    Some(egui::pos2(
        rect.left() + u * rect.width(),
        rect.top() + v * rect.height(),
    ))
}

fn world_radius_to_screen(settings: &TerrainProjectSettings, rect: egui::Rect, radius: f32) -> f32 {
    let world_width = (settings.world_bounds_max[0] - settings.world_bounds_min[0]).max(1.0);
    let scale = rect.width() / world_width;
    radius * scale
}

fn paint_layer_color(layer_index: usize) -> egui::Color32 {
    const COLORS: [egui::Color32; 6] = [
        egui::Color32::from_rgb(120, 200, 255),
        egui::Color32::from_rgb(255, 160, 80),
        egui::Color32::from_rgb(160, 220, 120),
        egui::Color32::from_rgb(240, 120, 200),
        egui::Color32::from_rgb(200, 200, 120),
        egui::Color32::from_rgb(160, 160, 240),
    ];
    COLORS[layer_index % COLORS.len()]
}

fn project_point(position: Vec3, view_proj: Mat4, rect: egui::Rect) -> Option<(egui::Pos2, f32)> {
    let clip = view_proj * Vec4::new(position.x, position.y, position.z, 1.0);
    if clip.w.abs() <= f32::EPSILON {
        return None;
    }
    let ndc = clip / clip.w;
    if ndc.z < -1.0 || ndc.z > 1.0 {
        return None;
    }
    let x = rect.left() + (ndc.x * 0.5 + 0.5) * rect.width();
    let y = rect.top() + (1.0 - (ndc.y * 0.5 + 0.5)) * rect.height();
    Some((egui::pos2(x, y), ndc.z))
}

fn project_bounds(
    min: Vec3,
    max: Vec3,
    view_proj: Mat4,
    rect: egui::Rect,
) -> Vec<(egui::Pos2, egui::Pos2)> {
    let corners = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    let mut projected = Vec::new();
    for corner in corners {
        projected.push(project_point(corner, view_proj, rect).map(|(pos, _)| pos));
    }
    let edges = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    let mut segments = Vec::new();
    for (a, b) in edges {
        if let (Some(start), Some(end)) = (projected[a], projected[b]) {
            segments.push((start, end));
        }
    }
    segments
}

fn lod_debug_color(lod: u8) -> egui::Color32 {
    const COLORS: [egui::Color32; 6] = [
        egui::Color32::from_rgb(110, 180, 255),
        egui::Color32::from_rgb(120, 230, 120),
        egui::Color32::from_rgb(255, 200, 120),
        egui::Color32::from_rgb(250, 140, 140),
        egui::Color32::from_rgb(200, 160, 255),
        egui::Color32::from_rgb(160, 200, 220),
    ];
    let idx = (lod as usize) % COLORS.len();
    COLORS[idx]
}

fn shade_color(base: egui::Color32, normal: Vec3, light_dir: Vec3) -> egui::Color32 {
    let brightness = normal.dot(light_dir).clamp(0.0, 1.0);
    let ambient = 0.25;
    let factor = ambient + (1.0 - ambient) * brightness;
    let r = (base.r() as f32 * factor).clamp(0.0, 255.0) as u8;
    let g = (base.g() as f32 * factor).clamp(0.0, 255.0) as u8;
    let b = (base.b() as f32 * factor).clamp(0.0, 255.0) as u8;
    egui::Color32::from_rgb(r, g, b)
}

fn material_palette_color(id: u32) -> egui::Color32 {
    let hash = id.wrapping_mul(0x9E3779B1).rotate_left(7);
    let r = 80 + (hash & 0x7F) as u8;
    let g = 80 + ((hash >> 8) & 0x7F) as u8;
    let b = 80 + ((hash >> 16) & 0x7F) as u8;
    egui::Color32::from_rgb(r, g, b)
}

fn material_color_for_vertex(mesh: &ChunkMeshResource, index: usize) -> egui::Color32 {
    let Some(ids) = mesh.material_ids.as_ref() else {
        return egui::Color32::from_rgb(120, 160, 190);
    };
    let Some(weights) = mesh.material_weights.as_ref() else {
        return egui::Color32::from_rgb(120, 160, 190);
    };
    let Some(id_set) = ids.get(index) else {
        return egui::Color32::from_rgb(120, 160, 190);
    };
    let Some(weight_set) = weights.get(index) else {
        return egui::Color32::from_rgb(120, 160, 190);
    };

    let mut r = 0.0;
    let mut g = 0.0;
    let mut b = 0.0;
    for (id, weight) in id_set.iter().zip(weight_set.iter()) {
        let color = material_palette_color(*id);
        r += *weight * color.r() as f32;
        g += *weight * color.g() as f32;
        b += *weight * color.b() as f32;
    }
    egui::Color32::from_rgb(r.clamp(0.0, 255.0) as u8, g.clamp(0.0, 255.0) as u8, b.clamp(0.0, 255.0) as u8)
}

fn triplanar_tint(base: egui::Color32, position: Vec3, normal: Vec3) -> egui::Color32 {
    let weights = normal.abs();
    let sum = (weights.x + weights.y + weights.z).max(0.0001);
    let wx = weights.x / sum;
    let wy = weights.y / sum;
    let wz = weights.z / sum;

    let x_variation = world_variation(position.y, position.z, 0.09);
    let y_variation = world_variation(position.x, position.z, 0.09);
    let z_variation = world_variation(position.x, position.y, 0.09);

    let x_color = scale_color(base, 0.85 + 0.15 * x_variation);
    let y_color = scale_color(base, 0.85 + 0.15 * y_variation);
    let z_color = scale_color(base, 0.85 + 0.15 * z_variation);

    blend_colors(blend_colors(x_color, y_color, wy / (wx + wy).max(0.0001)), z_color, wz)
}

fn blend_colors(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let r = a.r() as f32 + (b.r() as f32 - a.r() as f32) * t;
    let g = a.g() as f32 + (b.g() as f32 - a.g() as f32) * t;
    let b_val = a.b() as f32 + (b.b() as f32 - a.b() as f32) * t;
    egui::Color32::from_rgb(r as u8, g as u8, b_val as u8)
}

fn scale_color(color: egui::Color32, scale: f32) -> egui::Color32 {
    let scale = scale.clamp(0.0, 2.0);
    egui::Color32::from_rgb(
        (color.r() as f32 * scale).clamp(0.0, 255.0) as u8,
        (color.g() as f32 * scale).clamp(0.0, 255.0) as u8,
        (color.b() as f32 * scale).clamp(0.0, 255.0) as u8,
    )
}

fn world_variation(a: f32, b: f32, scale: f32) -> f32 {
    let value = (a * scale).sin() * 12.9898 + (b * scale).cos() * 78.233;
    value.sin().abs().clamp(0.0, 1.0)
}

fn collect_material_ids(
    artifact: &noren::rdb::terrain::TerrainChunkArtifact,
) -> Option<Vec<[u32; 4]>> {
    let ids = artifact.material_ids.as_ref()?;
    if ids.len() < artifact.vertices.len() * 4 {
        return None;
    }
    Some(
        ids.chunks(4)
            .take(artifact.vertices.len())
            .map(|chunk| [chunk[0], chunk[1], chunk[2], chunk[3]])
            .collect(),
    )
}

fn collect_material_weights(
    artifact: &noren::rdb::terrain::TerrainChunkArtifact,
) -> Option<Vec<[f32; 4]>> {
    let weights = artifact.material_weights.as_ref()?;
    if weights.len() < artifact.vertices.len() {
        return None;
    }
    Some(weights[..artifact.vertices.len()].to_vec())
}

fn chunk_coords_for_world(
    settings: &TerrainProjectSettings,
    world_x: f32,
    world_y: f32,
) -> (i32, i32) {
    let chunk_size_x = settings.tiles_per_chunk[0] as f32 * settings.tile_size;
    let chunk_size_y = settings.tiles_per_chunk[1] as f32 * settings.tile_size;
    let x = ((world_x - settings.world_bounds_min[0]) / chunk_size_x).floor() as i32;
    let y = ((world_y - settings.world_bounds_min[1]) / chunk_size_y).floor() as i32;
    (x, y)
}

fn height_from_artifact(
    settings: &TerrainProjectSettings,
    artifact: &noren::rdb::terrain::TerrainChunkArtifact,
    world_x: f32,
    world_y: f32,
) -> Option<f32> {
    let chunk_size_x = settings.tiles_per_chunk[0] as f32 * settings.tile_size;
    let chunk_size_y = settings.tiles_per_chunk[1] as f32 * settings.tile_size;
    let origin_x = artifact.chunk_coords[0] as f32 * chunk_size_x;
    let origin_y = artifact.chunk_coords[1] as f32 * chunk_size_y;
    let local_x = (world_x - origin_x) / settings.tile_size;
    let local_y = (world_y - origin_y) / settings.tile_size;

    if local_x < 0.0 || local_y < 0.0 {
        return None;
    }
    let grid_x = settings.tiles_per_chunk[0] + 1;
    let grid_y = settings.tiles_per_chunk[1] + 1;
    let max_x = grid_x.saturating_sub(1) as f32;
    let max_y = grid_y.saturating_sub(1) as f32;
    if local_x > max_x || local_y > max_y {
        return None;
    }

    let x0 = local_x.floor() as u32;
    let y0 = local_y.floor() as u32;
    let x1 = (x0 + 1).min(grid_x - 1);
    let y1 = (y0 + 1).min(grid_y - 1);
    let idx = |x: u32, y: u32| -> usize { (y * grid_x + x) as usize };
    let h00 = artifact.vertices.get(idx(x0, y0))?.position[2];
    let h10 = artifact.vertices.get(idx(x1, y0))?.position[2];
    let h01 = artifact.vertices.get(idx(x0, y1))?.position[2];
    let h11 = artifact.vertices.get(idx(x1, y1))?.position[2];
    let tx = local_x - x0 as f32;
    let ty = local_y - y0 as f32;
    let hx0 = h00 + (h10 - h00) * tx;
    let hx1 = h01 + (h11 - h01) * tx;
    Some(hx0 + (hx1 - hx0) * ty)
}

fn build_params_for_brush(
    tool: BrushTool,
    world_pos: [f32; 3],
    last_stamp_pos: Option<[f32; 3]>,
) -> (TerrainMutationOpKind, TerrainMutationParams) {
    match tool {
        BrushTool::SphereAdd => (
            TerrainMutationOpKind::SphereAdd,
            TerrainMutationParams::Sphere { center: world_pos },
        ),
        BrushTool::SphereSub => (
            TerrainMutationOpKind::SphereSub,
            TerrainMutationParams::Sphere { center: world_pos },
        ),
        BrushTool::CapsuleAdd => {
            let start = last_stamp_pos.unwrap_or(world_pos);
            (
                TerrainMutationOpKind::CapsuleAdd,
                TerrainMutationParams::Capsule {
                    start,
                    end: world_pos,
                },
            )
        }
        BrushTool::CapsuleSub => {
            let start = last_stamp_pos.unwrap_or(world_pos);
            (
                TerrainMutationOpKind::CapsuleSub,
                TerrainMutationParams::Capsule {
                    start,
                    end: world_pos,
                },
            )
        }
        BrushTool::Smooth => (
            TerrainMutationOpKind::Smooth,
            TerrainMutationParams::Smooth { center: world_pos },
        ),
        BrushTool::MaterialPaint => (
            TerrainMutationOpKind::MaterialPaint,
            TerrainMutationParams::MaterialPaint {
                center: world_pos,
                material_id: 0,
                blend_mode: noren::rdb::terrain::TerrainMaterialBlendMode::Blend,
            },
        ),
    }
}

fn affected_chunks_for_op(
    settings: &TerrainProjectSettings,
    op: &TerrainMutationOp,
) -> Vec<[i32; 2]> {
    let chunk_size_x = settings.tiles_per_chunk[0] as f32 * settings.tile_size;
    let chunk_size_y = settings.tiles_per_chunk[1] as f32 * settings.tile_size;
    if chunk_size_x <= 0.0 || chunk_size_y <= 0.0 {
        return Vec::new();
    }

    let (min_x, max_x, min_y, max_y, min_z, max_z) = match op.params {
        TerrainMutationParams::Sphere { center }
        | TerrainMutationParams::Smooth { center }
        | TerrainMutationParams::MaterialPaint {
            center,
            material_id: _,
            blend_mode: _,
        } => (
            center[0] - op.radius,
            center[0] + op.radius,
            center[1] - op.radius,
            center[1] + op.radius,
            center[2] - op.radius,
            center[2] + op.radius,
        ),
        TerrainMutationParams::Capsule { start, end } => (
            start[0].min(end[0]) - op.radius,
            start[0].max(end[0]) + op.radius,
            start[1].min(end[1]) - op.radius,
            start[1].max(end[1]) + op.radius,
            start[2].min(end[2]) - op.radius,
            start[2].max(end[2]) + op.radius,
        ),
    };

    if max_x < settings.world_bounds_min[0] || min_x > settings.world_bounds_max[0] {
        return Vec::new();
    }
    if max_y < settings.world_bounds_min[1] || min_y > settings.world_bounds_max[1] {
        return Vec::new();
    }
    let (max_chunk_x, max_chunk_y) = max_chunk_coords(settings, chunk_size_x, chunk_size_y);
    let (min_chunk_x, min_chunk_y) = chunk_coords_for_world(settings, min_x, min_y);
    let (max_chunk_x_raw, max_chunk_y_raw) = chunk_coords_for_world(settings, max_x, max_y);
    let min_chunk_x = min_chunk_x.clamp(0, max_chunk_x);
    let min_chunk_y = min_chunk_y.clamp(0, max_chunk_y);
    let max_chunk_x = max_chunk_x_raw.clamp(0, max_chunk_x);
    let max_chunk_y = max_chunk_y_raw.clamp(0, max_chunk_y);

    if min_z > settings.world_bounds_max[2] || max_z < settings.world_bounds_min[2] {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    for chunk_x in min_chunk_x..=max_chunk_x {
        for chunk_y in min_chunk_y..=max_chunk_y {
            let (aabb_min, aabb_max) =
                chunk_aabb(settings, chunk_x, chunk_y, chunk_size_x, chunk_size_y);
            let intersects = match op.params {
                TerrainMutationParams::Sphere { center }
                | TerrainMutationParams::Smooth { center }
                | TerrainMutationParams::MaterialPaint {
                    center,
                    material_id: _,
                    blend_mode: _,
                } => {
                    sphere_intersects_aabb(center, op.radius, aabb_min, aabb_max)
                }
                TerrainMutationParams::Capsule { start, end } => {
                    let (expanded_min, expanded_max) = expand_aabb(aabb_min, aabb_max, op.radius);
                    segment_intersects_aabb(start, end, expanded_min, expanded_max)
                }
            };
            if intersects {
                chunks.push([chunk_x, chunk_y]);
            }
        }
    }
    chunks
}

fn max_chunk_coords(
    settings: &TerrainProjectSettings,
    chunk_size_x: f32,
    chunk_size_y: f32,
) -> (i32, i32) {
    let world_size_x = (settings.world_bounds_max[0] - settings.world_bounds_min[0]).max(0.0);
    let world_size_y = (settings.world_bounds_max[1] - settings.world_bounds_min[1]).max(0.0);
    let count_x = (world_size_x / chunk_size_x).ceil().max(1.0) as i32;
    let count_y = (world_size_y / chunk_size_y).ceil().max(1.0) as i32;
    (count_x.saturating_sub(1), count_y.saturating_sub(1))
}

fn chunk_aabb(
    settings: &TerrainProjectSettings,
    chunk_x: i32,
    chunk_y: i32,
    chunk_size_x: f32,
    chunk_size_y: f32,
) -> ([f32; 3], [f32; 3]) {
    let min_x = settings.world_bounds_min[0] + chunk_x as f32 * chunk_size_x;
    let min_y = settings.world_bounds_min[1] + chunk_y as f32 * chunk_size_y;
    let min_z = settings.world_bounds_min[2];
    let max_x = min_x + chunk_size_x;
    let max_y = min_y + chunk_size_y;
    let max_z = settings.world_bounds_max[2];
    ([min_x, min_y, min_z], [max_x, max_y, max_z])
}

fn expand_aabb(min: [f32; 3], max: [f32; 3], radius: f32) -> ([f32; 3], [f32; 3]) {
    (
        [min[0] - radius, min[1] - radius, min[2] - radius],
        [max[0] + radius, max[1] + radius, max[2] + radius],
    )
}

fn sphere_intersects_aabb(
    center: [f32; 3],
    radius: f32,
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
) -> bool {
    let mut dist_sq = 0.0;
    for i in 0..3 {
        let c = center[i];
        if c < aabb_min[i] {
            let d = aabb_min[i] - c;
            dist_sq += d * d;
        } else if c > aabb_max[i] {
            let d = c - aabb_max[i];
            dist_sq += d * d;
        }
    }
    dist_sq <= radius * radius
}

fn segment_intersects_aabb(
    start: [f32; 3],
    end: [f32; 3],
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
) -> bool {
    let mut t_min: f32 = 0.0;
    let mut t_max: f32 = 1.0;
    let dir = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    for axis in 0..3 {
        let origin = start[axis];
        let direction = dir[axis];
        if direction.abs() < 1e-6 {
            if origin < aabb_min[axis] || origin > aabb_max[axis] {
                return false;
            }
        } else {
            let inv = 1.0 / direction;
            let mut t1 = (aabb_min[axis] - origin) * inv;
            let mut t2 = (aabb_max[axis] - origin) * inv;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            t_min = t_min.max(t1);
            t_max = t_max.min(t2);
            if t_min > t_max {
                return false;
            }
        }
    }
    true
}

fn mark_chunks_dirty(
    rdb: &mut RDBFile,
    project: &ProjectState,
    op: &TerrainMutationOp,
) -> Result<(), String> {
    let affected_chunks = affected_chunks_for_op(&project.settings, op);
    for chunk_coords in affected_chunks {
        let coord_key = chunk_coord_key(chunk_coords[0], chunk_coords[1]);
        let state_key = chunk_state_entry(&project.key, &coord_key);
        let mut state = rdb
            .fetch::<TerrainChunkState>(&state_key)
            .unwrap_or(TerrainChunkState {
                project_key: project.key.clone(),
                chunk_coords,
                dirty_flags: 0,
                dirty_reasons: Vec::new(),
                generator_version: project.settings.active_generator_version,
                mutation_version: project.settings.active_mutation_version,
                last_built_hashes: Vec::new(),
                dependency_hashes: TerrainChunkDependencyHashes {
                    settings_hash: 0,
                    generator_hash: 0,
                    mutation_hash: 0,
                },
            });
        state.dirty_flags |= TERRAIN_DIRTY_MUTATION;
        if !state
            .dirty_reasons
            .iter()
            .any(|reason| *reason == TerrainDirtyReason::MutationChanged)
        {
            state
                .dirty_reasons
                .push(TerrainDirtyReason::MutationChanged);
        }
        rdb.upsert(&state_key, &state).map_err(format_rdb_err)?;
    }
    Ok(())
}
