use std::{
    ptr::NonNull,
    time::{Duration, Instant},
};

use dashi::{Buffer, BufferInfo, BufferUsage, Context, Handle, MemoryVisibility};
use serde::{Deserialize, Serialize};

use super::{DatabaseEntry, primitives::Vertex};
use crate::{DataCache, RDBView, error::NorenError};

#[cfg(test)]
const UNLOAD_DELAY: Duration = Duration::from_secs(0);
#[cfg(not(test))]
const UNLOAD_DELAY: Duration = Duration::from_secs(5);

#[repr(C)]
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct GeometryLayer {
    pub vertices: Vec<Vertex>,
    pub indices: Option<Vec<u32>>,
}

#[repr(C)]
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct HostGeometry {
    pub vertices: Vec<Vertex>,
    pub indices: Option<Vec<u32>>,
    #[serde(default)]
    pub lods: Vec<GeometryLayer>,
}

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct DeviceGeometryLayer {
    pub vertices: Handle<Buffer>,
    pub indices: Handle<Buffer>,
}

#[derive(Clone, Debug, Default)]
pub struct DeviceGeometry {
    pub base: DeviceGeometryLayer,
    pub lods: Vec<DeviceGeometryLayer>,
}

pub struct GeometryDB {
    cache: DataCache<DeviceGeometry>,
    ctx: Option<NonNull<Context>>,
    data: Option<RDBView>,
}

impl GeometryDB {
    /// Creates a geometry database loader for the provided GPU context and module path.
    pub fn new(ctx: Option<*mut Context>, module_path: &str) -> Self {
        let data = match RDBView::load(module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        Self {
            data,
            ctx: ctx.and_then(NonNull::new),
            cache: Default::default(),
        }
    }

    pub fn import_ctx(&mut self, ctx: NonNull<Context>) {
        self.ctx = Some(ctx);
    }

    fn ctx_mut(&mut self) -> Result<&mut Context, NorenError> {
        self.ctx
            .as_mut()
            .map(|ctx| unsafe { ctx.as_mut() })
            .ok_or(NorenError::DashiContext())
    }

    /// Uploads host geometry into GPU buffers and caches the result.
    pub fn enter_gpu_geometry(
        &mut self,
        entry: DatabaseEntry<'_>,
        geom: HostGeometry,
    ) -> Result<DeviceGeometry, NorenError> {
        debug_assert!(self.cache.get(entry).is_none());

        let device_geom = if cfg!(test) {
            DeviceGeometry::default()
        } else {
            let HostGeometry {
                vertices,
                indices,
                lods,
            } = geom;
            let ctx = self.ctx_mut()?;

            let base = Self::upload_layer(ctx, entry, &GeometryLayer { vertices, indices })?;

            let lods = lods
                .into_iter()
                .enumerate()
                .map(|(idx, layer)| {
                    let debug_name = format!("{entry}::lod{idx}");
                    Self::upload_layer(ctx, &debug_name, &layer)
                })
                .collect::<Result<Vec<_>, _>>()?;

            DeviceGeometry { base, lods }
        };

        let cache_entry = self.cache.insert_or_increment(entry, || device_geom);

        Ok(cache_entry.payload.clone())
    }

    /// Returns whether the requested geometry entry is already cached on the GPU.
    pub fn is_loaded(&self, entry: &DatabaseEntry<'_>) -> bool {
        self.cache.get(*entry).is_some()
    }

    /// Retrieves host geometry data directly from the backing database file.
    pub fn fetch_raw_geometry(
        &mut self,
        entry: DatabaseEntry<'_>,
    ) -> Result<HostGeometry, NorenError> {
        if let Some(rdb) = &mut self.data {
            return Ok(rdb.fetch::<HostGeometry>(entry)?);
        }

        return Err(NorenError::DataFailure());
    }

    /// Ensures the geometry is loaded on the GPU and increments its reference count.
    pub fn fetch_gpu_geometry(
        &mut self,
        entry: DatabaseEntry<'_>,
    ) -> Result<DeviceGeometry, NorenError> {
        if !self.is_loaded(&entry) {
            let host_geom = self.fetch_raw_geometry(entry)?;
            return self.enter_gpu_geometry(entry, host_geom);
        }

        let cache_entry = self
            .cache
            .insert_or_increment(entry, || unreachable!("entry should already be loaded"));

        Ok(cache_entry.payload.clone())
    }

    /// Lists all geometry entries available in the backing database.
    pub fn enumerate_entries(&self) -> Vec<String> {
        self.data
            .as_ref()
            .map(|rdb| rdb.entries().into_iter().map(|meta| meta.name).collect())
            .unwrap_or_default()
    }

    /// Decrements a geometry reference, scheduling it for unloading after a delay.
    pub fn unref_entry(&mut self, entry: DatabaseEntry<'_>) -> Result<(), NorenError> {
        let unload_at = Instant::now() + UNLOAD_DELAY;
        match self.cache.decrement(entry, unload_at) {
            Some(_) => Ok(()),
            None => Err(NorenError::LookupFailure()),
        }
    }

    // Checks whether any geometry needs to be unloaded, and does so.
    /// Removes expired geometry buffers from the GPU and cache.
    pub fn unload_pulse(&mut self) {
        let expired = self.cache.drain_expired(Instant::now());

        if cfg!(test) {
            drop(expired);
            return;
        }

        if expired.is_empty() {
            return;
        }

        let Ok(ctx) = self.ctx_mut() else {
            return;
        };
        for (_key, entry) in expired {
            if entry.payload.base.vertices.valid() {
                ctx.destroy_buffer(entry.payload.base.vertices);
            }
            if entry.payload.base.indices.valid() {
                ctx.destroy_buffer(entry.payload.base.indices);
            }
            for lod in entry.payload.lods.iter() {
                if lod.vertices.valid() {
                    ctx.destroy_buffer(lod.vertices);
                }
                if lod.indices.valid() {
                    ctx.destroy_buffer(lod.indices);
                }
            }
        }
    }
}

impl GeometryDB {
    fn upload_layer(
        ctx: &mut Context,
        debug_name: &str,
        layer: &GeometryLayer,
    ) -> Result<DeviceGeometryLayer, NorenError> {
        let vertex_bytes = bytemuck::cast_slice(&layer.vertices);

        let vertex_buffer = ctx
            .make_buffer(&BufferInfo {
                debug_name,
                byte_size: vertex_bytes.len() as u32,
                visibility: MemoryVisibility::Gpu,
                usage: BufferUsage::VERTEX,
                initial_data: Some(vertex_bytes),
            })
            .map_err(|_| NorenError::UploadFailure())?;

        let index_handle = if let Some(indices) = &layer.indices {
            if indices.is_empty() {
                Handle::default()
            } else {
                let index_bytes = bytemuck::cast_slice(indices);
                let index_debug_name = format!("{debug_name}::indices");
                ctx.make_buffer(&BufferInfo {
                    debug_name: &index_debug_name,
                    byte_size: index_bytes.len() as u32,
                    visibility: MemoryVisibility::Gpu,
                    usage: BufferUsage::INDEX,
                    initial_data: Some(index_bytes),
                })
                .map_err(|_| NorenError::UploadFailure())?
            }
        } else {
            Handle::default()
        };

        Ok(DeviceGeometryLayer {
            vertices: vertex_buffer,
            indices: index_handle,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::rdbfile::RDBFile;

    fn sample_vertex(x: f32) -> Vertex {
        Vertex {
            position: [x, 0.0, 0.0],
            normal: [0.0, 1.0, 0.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            color: [1.0, 1.0, 1.0, 1.0],
        }
    }

    fn make_geometry_file(path: &std::path::Path, entry: &str) -> Result<(), NorenError> {
        let mut file = RDBFile::new();
        let host_geom = HostGeometry {
            vertices: vec![sample_vertex(0.0), sample_vertex(1.0), sample_vertex(2.0)],
            indices: Some(vec![0, 1, 2]),
            lods: Vec::new(),
        };
        file.add(entry, &host_geom)?;
        file.save(path)?;
        Ok(())
    }

    #[test]
    fn repeated_fetch_unref_cycle() -> Result<(), NorenError> {
        let entry = "geom/test_mesh";
        let tmp_path = std::env::temp_dir().join("test_geom.rdb");
        make_geometry_file(&tmp_path, entry)?;

        let view = RDBView::load(&tmp_path)?;
        let mut db = GeometryDB {
            cache: DataCache::default(),
            ctx: None,
            data: Some(view),
        };

        // First fetch should load from disk and cache
        db.fetch_gpu_geometry(entry)?;
        assert!(db.is_loaded(&entry));

        // Second fetch should bump refcount without loading again
        db.fetch_gpu_geometry(entry)?;

        // Release twice to drop all references, then unload
        db.unref_entry(entry)?;
        db.unref_entry(entry)?;
        db.unload_pulse();
        assert!(!db.is_loaded(&entry));

        // Repeat cycle to ensure subsequent loads work
        db.fetch_gpu_geometry(entry)?;
        db.unref_entry(entry)?;
        db.unload_pulse();
        assert!(!db.is_loaded(&entry));

        std::fs::remove_file(&tmp_path).ok();
        Ok(())
    }
}
