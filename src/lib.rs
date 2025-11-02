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
            let geometry = self.geometry.fetch_raw_geometry(geometry_entry)?;

            let mesh_name = mesh_def.name.clone().unwrap_or_else(|| mesh_key.clone());

            let mut mesh_textures = Vec::new();
            for tex_key in &mesh_def.textures {
                if let Some(tex_def) = layout.textures.get(tex_key) {
                    if tex_def.image.is_empty() {
                        continue;
                    }

                    let tex_entry = Self::leak_entry(&tex_def.image);
                    let image = self.imagery.fetch_raw_image(tex_entry)?;
                    let name = tex_def.name.clone().unwrap_or_else(|| tex_key.clone());
                    mesh_textures.push(HostTexture { name, image });
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
                            let image = self.imagery.fetch_raw_image(tex_entry)?;
                            let name = tex_def.name.clone().unwrap_or_else(|| tex_key.clone());
                            textures.push(HostTexture { name, image });
                        }
                    }

                    Some(HostMaterial {
                        name: material_def
                            .name
                            .clone()
                            .unwrap_or_else(|| material_key.clone()),
                        textures,
                    })
                } else {
                    None
                }
            } else {
                None
            };

            meshes.push(HostMesh {
                name: mesh_name,
                geometry,
                textures: mesh_textures,
                material,
            });
        }

        Ok(HostModel {
            name: model_name,
            meshes,
        })
    }

    pub fn fetch_gpu_model(&mut self, entry: DatabaseEntry) -> Result<DeviceModel, NorenError> {
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
            let geometry = self.geometry.fetch_gpu_geometry(geometry_entry)?;

            let mesh_name = mesh_def.name.clone().unwrap_or_else(|| mesh_key.clone());

            let mut mesh_textures = Vec::new();
            for tex_key in &mesh_def.textures {
                if let Some(tex_def) = layout.textures.get(tex_key) {
                    if tex_def.image.is_empty() {
                        continue;
                    }

                    let tex_entry = Self::leak_entry(&tex_def.image);
                    let image = self.imagery.fetch_gpu_image(tex_entry)?;
                    let name = tex_def.name.clone().unwrap_or_else(|| tex_key.clone());
                    mesh_textures.push(DeviceTexture { name, image });
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
                            let image = self.imagery.fetch_gpu_image(tex_entry)?;
                            let name = tex_def.name.clone().unwrap_or_else(|| tex_key.clone());
                            textures.push(DeviceTexture { name, image });
                        }
                    }

                    Some(DeviceMaterial {
                        name: material_def
                            .name
                            .clone()
                            .unwrap_or_else(|| material_key.clone()),
                        textures,
                    })
                } else {
                    None
                }
            } else {
                None
            };

            meshes.push(DeviceMesh {
                name: mesh_name,
                geometry,
                textures: mesh_textures,
                material,
            });
        }

        Ok(DeviceModel {
            name: model_name,
            meshes,
        })
    }
}

impl DB {
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
    use crate::utils::rdbfile::RDBFile;
    use std::{fs, path::Path};
    use tempfile::tempdir;

    const MODEL_ENTRY: DatabaseEntry = "model/simple";
    const GEOMETRY_ENTRY: &str = "geom/simple_mesh";
    const IMAGE_ENTRY: &str = "imagery/sample_texture";

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

        let models_src = Path::new("sample/sample_pre/models.json");
        let models_dst = base_dir.join("models.json");
        fs::copy(models_src, &models_dst).expect("copy models.json");

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
        assert_eq!(host_model.meshes.len(), 1);
        let mesh = &host_model.meshes[0];
        assert_eq!(mesh.geometry.vertices.len(), 3);
        assert!(mesh.material.is_some());
        let mat = mesh.material.as_ref().unwrap();
        assert_eq!(mat.textures.len(), 1);
        assert_eq!(mesh.textures.len(), 0);

        let device_model = db.fetch_gpu_model(MODEL_ENTRY)?;
        assert_eq!(device_model.meshes.len(), 1);
        let device_mesh = &device_model.meshes[0];
        assert!(device_mesh.material.is_some());
        assert_eq!(device_mesh.material.as_ref().unwrap().textures.len(), 1);

        Ok(())
    }
}
