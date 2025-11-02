pub mod datatypes;
pub mod meta;
mod parsing;
mod utils;

use std::io::ErrorKind;

use datatypes::*;
use error::NorenError;
use meta::*;
use parsing::*;
use utils::*;

pub use parsing::DatabaseLayoutFile;
pub use utils::error::RdbErr;
pub use utils::rdbfile::RDBFile;

pub struct DBInfo<'a> {
    pub ctx: *mut dashi::Context,
    pub base_dir: &'a str,
    pub layout_file: Option<&'a str>,
}

pub struct DB {
    geometry: GeometryDB,
    imagery: ImageDB,
    model_file: Option<ModelLayoutFile>,
}

////////////////////////////////////////////////
/// NorenDB (Noran Database)
/// * Provides readonly access to reading and loading data
///   from a Noren Generated Database.
///
/// * Handles access to Host(CPU) and Device(GPU) data.
/// ** CPU Data is read from the mapped memory when requested.
/// ** GPU Data is GPU-Ready (OK for usage/uploads), cached and refcounted when requested, and will unload on a timer when all refs
///    are released. The timer is so that if data is quickly unreffed/fetched, it will handle
///    gracefully. This timer is configurable.
////////////////////////////////////////////////

impl DB {
    pub fn new(info: &DBInfo) -> Result<Self, NorenError> {
        let layout: DatabaseLayoutFile = match info.layout_file {
            Some(f) => serde_json::from_str(&std::fs::read_to_string(f.to_string())?)?,
            None => Default::default(),
        };

        let geometry = GeometryDB::new(info.ctx, &format!("{}/{}", info.base_dir, layout.geometry));
        let imagery = ImageDB::new(info.ctx, &format!("{}/{}", info.base_dir, layout.imagery));
        let model_path = format!("{}/{}", info.base_dir, layout.models);
        let model_file = match std::fs::read_to_string(&model_path) {
            Ok(raw) if raw.trim().is_empty() => None,
            Ok(raw) => Some(serde_json::from_str(&raw)?),
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };

        Ok(Self {
            geometry,
            imagery,
            model_file,
        })
    }

    pub fn geometry(&self) -> &GeometryDB {
        &self.geometry
    }

    pub fn geometry_mut(&mut self) -> &mut GeometryDB {
        &mut self.geometry
    }

    pub fn imagery(&self) -> &ImageDB {
        &self.imagery
    }

    pub fn imagery_mut(&mut self) -> &mut ImageDB {
        &mut self.imagery
    }

    pub fn font(&self) -> &FontDB {
        todo!()
    }

    pub fn fetch_model(&mut self, entry: DatabaseEntry) -> Result<HostModel, NorenError> {
        self.assemble_model(
            entry,
            |geometry_db, entry| geometry_db.fetch_raw_geometry(entry),
            |imagery_db, entry| imagery_db.fetch_raw_image(entry),
            |name, image| HostTexture { name, image },
            |name, textures| HostMaterial { name, textures },
            |name, geometry, textures, material| HostMesh {
                name,
                geometry,
                textures,
                material,
            },
            |name, meshes| HostModel { name, meshes },
        )
    }

    pub fn fetch_gpu_model(&mut self, entry: DatabaseEntry) -> Result<DeviceModel, NorenError> {
        self.assemble_model(
            entry,
            |geometry_db, entry| geometry_db.fetch_gpu_geometry(entry),
            |imagery_db, entry| imagery_db.fetch_gpu_image(entry),
            |name, image| DeviceTexture { name, image },
            |name, textures| DeviceMaterial { name, textures },
            |name, geometry, textures, material| DeviceMesh {
                name,
                geometry,
                textures,
                material,
            },
            |name, meshes| DeviceModel { name, meshes },
        )
    }
}

impl DB {
    fn assemble_model<
        Model,
        Mesh,
        Material,
        Texture,
        Geometry,
        Image,
        FetchGeometry,
        FetchImage,
        MakeTexture,
        MakeMaterial,
        MakeMesh,
        MakeModel,
    >(
        &mut self,
        entry: DatabaseEntry,
        mut fetch_geometry: FetchGeometry,
        mut fetch_image: FetchImage,
        mut make_texture: MakeTexture,
        mut make_material: MakeMaterial,
        mut make_mesh: MakeMesh,
        mut make_model: MakeModel,
    ) -> Result<Model, NorenError>
    where
        FetchGeometry: FnMut(&mut GeometryDB, DatabaseEntry) -> Result<Geometry, NorenError>,
        FetchImage: FnMut(&mut ImageDB, DatabaseEntry) -> Result<Image, NorenError>,
        MakeTexture: FnMut(String, Image) -> Texture,
        MakeMaterial: FnMut(String, Vec<Texture>) -> Material,
        MakeMesh: FnMut(String, Geometry, Vec<Texture>, Option<Material>) -> Mesh,
        MakeModel: FnMut(String, Vec<Mesh>) -> Model,
    {
        let layout = self
            .model_file
            .as_ref()
            .ok_or_else(|| NorenError::LookupFailure())?;

        let model = layout
            .models
            .get(entry)
            .ok_or_else(|| NorenError::LookupFailure())?;

        let model_name = model.name.clone().unwrap_or_else(|| entry.to_string());
        let mut meshes = Vec::new();

        for mesh_key in &model.meshes {
            let mesh_def = match layout.meshes.get(mesh_key) {
                Some(mesh) => mesh,
                None => continue,
            };

            if mesh_def.geometry.is_empty() {
                continue;
            }

            let geometry_entry = Self::leak_entry(&mesh_def.geometry);
            let geometry = fetch_geometry(&mut self.geometry, geometry_entry)?;

            let mesh_name = mesh_def.name.clone().unwrap_or_else(|| mesh_key.clone());

            let mut mesh_textures = Vec::new();
            for tex_key in &mesh_def.textures {
                if let Some(tex_def) = layout.textures.get(tex_key) {
                    if tex_def.image.is_empty() {
                        continue;
                    }

                    let tex_entry = Self::leak_entry(&tex_def.image);
                    let image = fetch_image(&mut self.imagery, tex_entry)?;
                    let name = tex_def.name.clone().unwrap_or_else(|| tex_key.clone());
                    mesh_textures.push(make_texture(name, image));
                }
            }

            let material = if let Some(material_key) = &mesh_def.material {
                if let Some(material_def) = layout.materials.get(material_key) {
                    let mut textures = Vec::new();
                    for tex_key in &material_def.textures {
                        if let Some(tex_def) = layout.textures.get(tex_key) {
                            if tex_def.image.is_empty() {
                                continue;
                            }

                            let tex_entry = Self::leak_entry(&tex_def.image);
                            let image = fetch_image(&mut self.imagery, tex_entry)?;
                            let name = tex_def.name.clone().unwrap_or_else(|| tex_key.clone());
                            textures.push(make_texture(name, image));
                        }
                    }

                    Some(make_material(
                        material_def
                            .name
                            .clone()
                            .unwrap_or_else(|| material_key.clone()),
                        textures,
                    ))
                } else {
                    None
                }
            } else {
                None
            };

            meshes.push(make_mesh(mesh_name, geometry, mesh_textures, material));
        }

        Ok(make_model(model_name, meshes))
    }

    fn leak_entry(entry: &str) -> DatabaseEntry {
        Box::leak(entry.to_string().into_boxed_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datatypes::{
        geometry::HostGeometry,
        imagery::{HostImage, ImageInfo},
        primitives::Vertex,
    };
    use crate::parsing::{MaterialLayout, MeshLayout, ModelLayout, ModelLayoutFile, TextureLayout};
    use crate::utils::rdbfile::RDBFile;
    use std::fs::File;
    use tempfile::tempdir;

    const MODEL_ENTRY: DatabaseEntry = "model/simple";
    const GEOMETRY_ENTRY: &str = "geom/simple_mesh";
    const IMAGE_ENTRY: &str = "imagery/sample_texture";
    const MATERIAL_ENTRY: &str = "material/simple";
    const MESH_ENTRY: &str = "mesh/simple_mesh";
    const MESH_MISSING_GEOMETRY: &str = "mesh/no_geometry";
    const MESH_TEXTURE_ENTRY: &str = "texture/mesh_texture";
    const MISSING_TEXTURE_ENTRY: &str = "texture/missing_image";

    fn sample_vertex(x: f32) -> Vertex {
        Vertex {
            position: [x, 0.0, 0.0],
            normal: [0.0, 1.0, 0.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            color: [1.0, 1.0, 1.0, 1.0],
        }
    }

    #[test]
    fn load_sample_model_definition() -> Result<(), NorenError> {
        let tmp = tempdir().expect("create temp dir");
        let base_dir = tmp.path();

        let models_dst = base_dir.join("models.json");
        let mut layout = ModelLayoutFile::default();

        layout.textures.insert(
            MESH_TEXTURE_ENTRY.to_string(),
            TextureLayout {
                image: IMAGE_ENTRY.to_string(),
                name: None,
            },
        );
        layout.textures.insert(
            MISSING_TEXTURE_ENTRY.to_string(),
            TextureLayout {
                image: String::new(),
                name: Some("ShouldSkip".to_string()),
            },
        );

        layout.materials.insert(
            MATERIAL_ENTRY.to_string(),
            MaterialLayout {
                name: None,
                textures: vec![
                    MESH_TEXTURE_ENTRY.to_string(),
                    MISSING_TEXTURE_ENTRY.to_string(),
                ],
            },
        );

        layout.meshes.insert(
            MESH_ENTRY.to_string(),
            MeshLayout {
                name: Some("Simple Mesh".to_string()),
                geometry: GEOMETRY_ENTRY.to_string(),
                material: Some(MATERIAL_ENTRY.to_string()),
                textures: vec![
                    MESH_TEXTURE_ENTRY.to_string(),
                    MISSING_TEXTURE_ENTRY.to_string(),
                ],
            },
        );

        layout.meshes.insert(
            MESH_MISSING_GEOMETRY.to_string(),
            MeshLayout {
                name: Some("No Geometry".to_string()),
                geometry: String::new(),
                material: None,
                textures: vec![MESH_TEXTURE_ENTRY.to_string()],
            },
        );

        layout.models.insert(
            MODEL_ENTRY.to_string(),
            ModelLayout {
                name: None,
                meshes: vec![MESH_ENTRY.to_string(), MESH_MISSING_GEOMETRY.to_string()],
            },
        );

        serde_json::to_writer(File::create(&models_dst)?, &layout)?;

        let mut geom_rdb = RDBFile::new();
        let geom = HostGeometry {
            vertices: vec![sample_vertex(0.0), sample_vertex(1.0), sample_vertex(2.0)],
            indices: Some(vec![0, 1, 2]),
        };
        geom_rdb.add(GEOMETRY_ENTRY, &geom)?;
        geom_rdb.save(base_dir.join("geometry.rdb"))?;

        let image_info = ImageInfo {
            name: IMAGE_ENTRY.to_string(),
            dim: [1, 1, 1],
            layers: 1,
            format: dashi::Format::RGBA8,
            mip_levels: 1,
        };
        let host_image = HostImage {
            info: image_info,
            data: vec![255, 255, 255, 255],
        };
        let mut img_rdb = RDBFile::new();
        img_rdb.add(IMAGE_ENTRY, &host_image)?;
        img_rdb.save(base_dir.join("imagery.rdb"))?;

        let mut ctx =
            dashi::Context::headless(&Default::default()).expect("create headless context");

        let db_info = DBInfo {
            ctx: &mut ctx,
            base_dir: base_dir.to_str().expect("base dir to str"),
            layout_file: None,
        };

        let mut db = DB::new(&db_info)?;

        let host_model = db.fetch_model(MODEL_ENTRY)?;
        assert_eq!(host_model.name, MODEL_ENTRY);
        assert_eq!(host_model.meshes.len(), 1);
        let mesh = &host_model.meshes[0];
        assert_eq!(mesh.name, "Simple Mesh");
        assert_eq!(mesh.geometry.vertices.len(), 3);
        assert_eq!(mesh.textures.len(), 1);
        assert_eq!(mesh.textures[0].name, MESH_TEXTURE_ENTRY);
        assert!(mesh.material.is_some());
        let mat = mesh.material.as_ref().unwrap();
        assert_eq!(mat.name, MATERIAL_ENTRY);
        assert_eq!(mat.textures.len(), 1);
        assert_eq!(mat.textures[0].name, MESH_TEXTURE_ENTRY);

        let device_model = db.fetch_gpu_model(MODEL_ENTRY)?;
        assert_eq!(device_model.name, MODEL_ENTRY);
        assert_eq!(device_model.meshes.len(), 1);
        let device_mesh = &device_model.meshes[0];
        assert_eq!(device_mesh.name, "Simple Mesh");
        assert!(device_mesh.material.is_some());
        let device_mat = device_mesh.material.as_ref().unwrap();
        assert_eq!(device_mat.name, MATERIAL_ENTRY);
        assert_eq!(device_mesh.textures.len(), 1);
        assert_eq!(device_mesh.textures[0].name, MESH_TEXTURE_ENTRY);
        assert_eq!(device_mat.textures.len(), 1);
        assert_eq!(device_mat.textures[0].name, MESH_TEXTURE_ENTRY);

        Ok(())
    }
}
