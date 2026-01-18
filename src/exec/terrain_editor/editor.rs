use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use eframe::egui;
use noren::rdb::terrain::{
    TERRAIN_DIRTY_MUTATION, TerrainChunkDependencyHashes, TerrainChunkState, TerrainDirtyReason,
    TerrainGeneratorDefinition, TerrainMutationLayer, TerrainMutationOp, TerrainMutationOpKind,
    TerrainMutationParams, TerrainProjectSettings, chunk_artifact_entry, chunk_coord_key,
    chunk_state_entry, generator_entry, mutation_layer_entry, mutation_op_entry,
    project_settings_entry,
};
use noren::terrain::sample_height_with_mutations;
use noren::{RDBEntryMeta, RDBFile, RdbErr};

#[derive(Clone, Debug)]
struct ChunkArtifactInfo {
    entry: String,
    region: String,
    coord: Option<(i32, i32)>,
    lod: Option<u8>,
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
}

#[derive(Clone, Debug, Default)]
struct ViewportState {
    last_stamp_time: Option<f64>,
    last_stamp_pos: Option<[f32; 3]>,
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
    log: Vec<String>,
    validation: Vec<String>,
    last_error: Option<String>,
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
            },
            viewport: ViewportState::default(),
            log: Vec::new(),
            validation: Vec::new(),
            last_error: None,
        }
    }
}

impl TerrainEditorApp {
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("RDB:");
            ui.text_edit_singleline(&mut self.rdb_path_input);
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("RDB", &["rdb"])
                    .pick_file()
                {
                    self.rdb_path_input = path.display().to_string();
                }
            }
            if ui.button("Init RDB").clicked() {
                if let Err(err) = self.init_rdb_from_input() {
                    self.set_error(err);
                }
            }
            if ui.button("Load RDB").clicked() {
                if let Err(err) = self.load_rdb_from_input() {
                    self.set_error(err);
                }
            }
        });

        ui.horizontal(|ui| {
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
            if ui.button("Open Project").clicked() {
                if let Err(err) = self.open_project_from_input() {
                    self.set_error(err);
                }
            }
            if ui.button("New Project").clicked() {
                if let Err(err) = self.create_project_from_input() {
                    self.set_error(err);
                }
            }
        });

        if let Some(error) = self.last_error.take() {
            ui.colored_label(egui::Color32::RED, error);
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

        self.draw_brush_controls(ui);
        ui.add_space(8.0);

        let available = ui.available_size();
        let viewport_height = available.y.max(220.0);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(available.x, viewport_height),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, egui::Color32::from_gray(20));
        painter.rect_stroke(rect, 0.0, (1.0, egui::Color32::DARK_GRAY));

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
            if let Some(info) = parse_project_chunk(&entry.name, project_key) {
                artifacts.push(info);
            }
        } else if entry.name.starts_with("terrain/chunk_") {
            if let Some(info) = parse_legacy_chunk(&entry.name) {
                artifacts.push(info);
            }
        }
    }
    artifacts
}

fn parse_project_chunk(name: &str, project_key: &str) -> Option<ChunkArtifactInfo> {
    let prefix = format!("terrain/chunk_artifact/{project_key}/");
    let remainder = name.strip_prefix(&prefix)?;
    let mut parts = remainder.split('/');
    let coord_part = parts.next()?;
    let lod_part = parts.next()?;
    Some(ChunkArtifactInfo {
        entry: name.to_string(),
        region: "default".to_string(),
        coord: parse_coord(coord_part),
        lod: parse_lod(lod_part),
    })
}

fn parse_legacy_chunk(name: &str) -> Option<ChunkArtifactInfo> {
    let remainder = name.strip_prefix("terrain/chunk_")?;
    Some(ChunkArtifactInfo {
        entry: name.to_string(),
        region: "legacy".to_string(),
        coord: parse_coord(remainder),
        lod: None,
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

fn format_rdb_err(err: RdbErr) -> String {
    format!("RDB error: {err}")
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

fn world_radius_to_screen(settings: &TerrainProjectSettings, rect: egui::Rect, radius: f32) -> f32 {
    let world_width = (settings.world_bounds_max[0] - settings.world_bounds_min[0]).max(1.0);
    let scale = rect.width() / world_width;
    radius * scale
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
            },
        ),
    }
}

fn mark_chunks_dirty(
    rdb: &mut RDBFile,
    project: &ProjectState,
    op: &TerrainMutationOp,
) -> Result<(), String> {
    let (min_x, max_x, min_y, max_y) = match op.params {
        TerrainMutationParams::Sphere { center }
        | TerrainMutationParams::Smooth { center }
        | TerrainMutationParams::MaterialPaint { center, .. } => {
            let min_x = center[0] - op.radius;
            let max_x = center[0] + op.radius;
            let min_y = center[1] - op.radius;
            let max_y = center[1] + op.radius;
            (min_x, max_x, min_y, max_y)
        }
        TerrainMutationParams::Capsule { start, end } => {
            let min_x = start[0].min(end[0]) - op.radius;
            let max_x = start[0].max(end[0]) + op.radius;
            let min_y = start[1].min(end[1]) - op.radius;
            let max_y = start[1].max(end[1]) + op.radius;
            (min_x, max_x, min_y, max_y)
        }
    };
    let (min_chunk_x, min_chunk_y) = chunk_coords_for_world(&project.settings, min_x, min_y);
    let (max_chunk_x, max_chunk_y) = chunk_coords_for_world(&project.settings, max_x, max_y);
    for chunk_x in min_chunk_x..=max_chunk_x {
        for chunk_y in min_chunk_y..=max_chunk_y {
            let coord_key = chunk_coord_key(chunk_x, chunk_y);
            let state_key = chunk_state_entry(&project.key, &coord_key);
            let mut state =
                rdb.fetch::<TerrainChunkState>(&state_key)
                    .unwrap_or(TerrainChunkState {
                        project_key: project.key.clone(),
                        chunk_coords: [chunk_x, chunk_y],
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
    }
    Ok(())
}
