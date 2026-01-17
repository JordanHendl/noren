use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use eframe::egui;
use noren::{RDBEntryMeta, RDBFile, RdbErr};
use serde::Deserialize as SerdeDeserialize;
use serde::Serialize;

#[derive(Clone, Debug, Serialize, SerdeDeserialize)]
struct TerrainProjectSettings {
    name: String,
    seed: u64,
    tile_size: f32,
    tiles_per_chunk: [u32; 2],
}

impl Default for TerrainProjectSettings {
    fn default() -> Self {
        Self {
            name: "New Terrain Project".to_string(),
            seed: 1337,
            tile_size: 1.0,
            tiles_per_chunk: [32, 32],
        }
    }
}

#[derive(Clone, Debug, Serialize, SerdeDeserialize)]
struct TerrainGeneratorSettings {
    algorithm: String,
    frequency: f32,
    amplitude: f32,
}

impl Default for TerrainGeneratorSettings {
    fn default() -> Self {
        Self {
            algorithm: "ridge-noise".to_string(),
            frequency: 0.02,
            amplitude: 64.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, SerdeDeserialize)]
struct TerrainMutationLayer {
    name: String,
    enabled: bool,
    weight: f32,
}

impl TerrainMutationLayer {
    fn with_index(idx: usize) -> Self {
        Self {
            name: format!("Layer {}", idx),
            enabled: true,
            weight: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
struct ChunkArtifactInfo {
    entry: String,
    region: String,
    coord: Option<(i32, i32)>,
}

#[derive(Clone, Debug)]
struct ProjectState {
    rdb_path: PathBuf,
    key: String,
    settings: TerrainProjectSettings,
    generator: TerrainGeneratorSettings,
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

pub struct TerrainEditorApp {
    rdb_path_input: String,
    project_key_input: String,
    project_keys: Vec<String>,
    rdb: Option<RDBFile>,
    project: Option<ProjectState>,
    selection: Selection,
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

        let settings = match rdb.fetch::<TerrainProjectSettings>(&settings_entry(&project_key)) {
            Ok(value) => value,
            Err(_) => {
                missing_logs.push("Project settings missing; using defaults.".to_string());
                TerrainProjectSettings::default()
            }
        };
        let generator = match rdb.fetch::<TerrainGeneratorSettings>(&generator_entry(&project_key))
        {
            Ok(value) => value,
            Err(_) => {
                missing_logs.push("Generator settings missing; using defaults.".to_string());
                TerrainGeneratorSettings::default()
            }
        };
        let mutation_layers =
            match rdb.fetch::<Vec<TerrainMutationLayer>>(&mutation_layers_entry(&project_key)) {
                Ok(value) => value,
                Err(_) => {
                    missing_logs.push("Mutation layers missing; using defaults.".to_string());
                    Vec::new()
                }
            };

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

        let settings = TerrainProjectSettings {
            name: format!("Terrain Project {project_key}"),
            ..TerrainProjectSettings::default()
        };
        let generator = TerrainGeneratorSettings::default();
        let mutation_layers = vec![TerrainMutationLayer::with_index(1)];

        rdb.upsert(&settings_entry(&project_key), &settings)
            .map_err(format_rdb_err)?;
        rdb.upsert(&generator_entry(&project_key), &generator)
            .map_err(format_rdb_err)?;
        rdb.upsert(&mutation_layers_entry(&project_key), &mutation_layers)
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
        self.log("Created new project and saved to RDB.");
        Ok(())
    }

    fn draw_tree(&mut self, ui: &mut egui::Ui) {
        ui.heading("Project Tree");
        let (mut add_layer_clicked, mut new_selection) = (false, None);
        let (project_key, layer_names, chunk_regions) = if let Some(project) = &self.project {
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
                if let Some(project) = &mut self.project {
                    if let Some(layer) = project.mutation_layers.get_mut(idx) {
                        ui.heading("Mutation layer");
                        changed |= ui.text_edit_singleline(&mut layer.name).changed();
                        changed |= ui.checkbox(&mut layer.enabled, "Enabled").changed();
                        ui.horizontal(|ui| {
                            ui.label("Weight");
                            changed |= ui
                                .add(egui::DragValue::new(&mut layer.weight).speed(0.05))
                                .changed();
                        });
                        if ui.button("Delete layer").clicked() {
                            project.mutation_layers.remove(idx);
                            self.selection = Selection::MutationLayer(idx.saturating_sub(1));
                            changed = true;
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

        rdb.upsert(&settings_entry(&project.key), &project.settings)
            .map_err(format_rdb_err)?;
        rdb.upsert(&generator_entry(&project.key), &project.generator)
            .map_err(format_rdb_err)?;
        rdb.upsert(
            &mutation_layers_entry(&project.key),
            &project.mutation_layers,
        )
        .map_err(format_rdb_err)?;
        rdb.save(&project.rdb_path).map_err(format_rdb_err)?;

        project.chunks = collect_chunk_artifacts(&rdb.entries(), &project.key);
        self.log("Saved project data to RDB.");
        Ok(())
    }

    fn add_layer(&mut self) {
        if let Some(project) = &mut self.project {
            let idx = project.mutation_layers.len() + 1;
            project
                .mutation_layers
                .push(TerrainMutationLayer::with_index(idx));
            self.selection = Selection::MutationLayer(project.mutation_layers.len() - 1);
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

        egui::CentralPanel::default().show(ctx, |ui| {
            self.draw_details(ui);
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
    let prefix = format!("terrain/project/{project_key}/");
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
    let prefix = format!("terrain/project/{project_key}/");
    let remainder = name.strip_prefix(&prefix)?;
    if let Some(remainder) = remainder.strip_prefix("chunk/") {
        let mut parts = remainder.split('/');
        let region = parts.next()?.to_string();
        let coord_part = parts.next()?;
        let coord = parse_coord(coord_part);
        Some(ChunkArtifactInfo {
            entry: name.to_string(),
            region,
            coord,
        })
    } else if let Some(remainder) = remainder.strip_prefix("chunk_") {
        Some(ChunkArtifactInfo {
            entry: name.to_string(),
            region: "default".to_string(),
            coord: parse_coord(remainder),
        })
    } else {
        None
    }
}

fn parse_legacy_chunk(name: &str) -> Option<ChunkArtifactInfo> {
    let remainder = name.strip_prefix("terrain/chunk_")?;
    Some(ChunkArtifactInfo {
        entry: name.to_string(),
        region: "legacy".to_string(),
        coord: parse_coord(remainder),
    })
}

fn parse_coord(value: &str) -> Option<(i32, i32)> {
    let mut parts = value.split('_');
    let x = parts.next()?.parse().ok()?;
    let y = parts.next()?.parse().ok()?;
    Some((x, y))
}

fn settings_entry(project_key: &str) -> String {
    format!("terrain/project/{project_key}/settings")
}

fn generator_entry(project_key: &str) -> String {
    format!("terrain/project/{project_key}/generator")
}

fn mutation_layers_entry(project_key: &str) -> String {
    format!("terrain/project/{project_key}/mutation_layers")
}

fn format_rdb_err(err: RdbErr) -> String {
    format!("RDB error: {err}")
}
