use std::{
    collections::{HashMap, HashSet},
    ptr::NonNull,
    time::{Duration, Instant},
};

use dashi::{Buffer, BufferInfo, BufferUsage, BufferView, Context, Handle, MemoryVisibility};
use serde::{Deserialize, Serialize};

use super::{DatabaseEntry, primitives::Vertex};
use crate::{DataCache, RDBView, defaults::default_primitives, error::NorenError};

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
    pub vertices: GeometryBufferRef,
    pub indices: GeometryBufferRef,
}

#[derive(Clone, Debug, Default)]
pub struct DeviceGeometry {
    pub base: DeviceGeometryLayer,
    pub lods: Vec<DeviceGeometryLayer>,
}

pub struct GeometryDBBuilder {
    ctx: Option<*mut Context>,
    module_path: String,
    pooled_uploads: bool,
}

impl GeometryDBBuilder {
    pub fn new(ctx: Option<*mut Context>, module_path: &str) -> Self {
        Self {
            ctx,
            module_path: module_path.to_string(),
            pooled_uploads: false,
        }
    }

    pub fn pooled_uploads(mut self, enable: bool) -> Self {
        self.pooled_uploads = enable;
        self
    }

    pub fn build(self) -> GeometryDB {
        let data = match RDBView::load(&self.module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        GeometryDB {
            data,
            ctx: self.ctx.and_then(NonNull::new),
            cache: Default::default(),
            defaults: default_primitives().into_iter().collect(),
            pooled_uploads: self.pooled_uploads,
            vertex_pool: GeometryUploadPool::new(BufferUsage::VERTEX, "geometry::vertices"),
            index_pool: GeometryUploadPool::new(BufferUsage::INDEX, "geometry::indices"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GeometryBufferSlice {
    pub buffer: Handle<Buffer>,
    pub offset: u32,
    pub size: u32,
}

#[derive(Clone, Debug)]
pub enum GeometryBufferRef {
    Dedicated(Handle<Buffer>),
    Slice(GeometryBufferSlice),
    None,
}

impl Default for GeometryBufferRef {
    fn default() -> Self {
        Self::None
    }
}

impl GeometryBufferRef {
    pub fn handle(&self) -> Option<Handle<Buffer>> {
        match self {
            GeometryBufferRef::Dedicated(handle) => handle.valid().then_some(*handle),
            GeometryBufferRef::Slice(slice) => slice.buffer.valid().then_some(slice.buffer),
            GeometryBufferRef::None => None,
        }
    }

    fn replace_handle(&mut self, old: Handle<Buffer>, new: Handle<Buffer>) {
        match self {
            GeometryBufferRef::Dedicated(handle) if *handle == old => *handle = new,
            GeometryBufferRef::Slice(slice) if slice.buffer == old => slice.buffer = new,
            _ => {}
        }
    }
}

impl DeviceGeometryLayer {
    fn push_handles(&self, handles: &mut HashSet<Handle<Buffer>>) {
        if let Some(handle) = self.vertices.handle() {
            handles.insert(handle);
        }
        if let Some(handle) = self.indices.handle() {
            handles.insert(handle);
        }
    }

    fn replace_handles(&mut self, old: Handle<Buffer>, new: Handle<Buffer>) {
        self.vertices.replace_handle(old, new);
        self.indices.replace_handle(old, new);
    }
}

impl DeviceGeometry {
    pub fn buffer_handles(&self) -> Vec<Handle<Buffer>> {
        let mut handles = HashSet::new();
        self.base.push_handles(&mut handles);
        for lod in &self.lods {
            lod.push_handles(&mut handles);
        }
        handles.into_iter().collect()
    }

    fn replace_handles(&mut self, old: Handle<Buffer>, new: Handle<Buffer>) {
        self.base.replace_handles(old, new);
        for lod in &mut self.lods {
            lod.replace_handles(old, new);
        }
    }
}

pub struct GeometryDB {
    cache: DataCache<DeviceGeometry>,
    ctx: Option<NonNull<Context>>,
    data: Option<RDBView>,
    defaults: HashMap<String, HostGeometry>,
    pooled_uploads: bool,
    vertex_pool: GeometryUploadPool,
    index_pool: GeometryUploadPool,
}

#[derive(Default)]
struct GeometryUploadPool {
    buffer: Handle<Buffer>,
    capacity: u32,
    data: Vec<u8>,
    usage: BufferUsage,
    debug_name: String,
}

impl GeometryUploadPool {
    fn new(usage: BufferUsage, debug_name: &str) -> Self {
        Self {
            buffer: Handle::default(),
            capacity: 0,
            data: Vec::new(),
            usage,
            debug_name: debug_name.to_string(),
        }
    }

    fn buffer_handle(&self) -> Handle<Buffer> {
        self.buffer
    }

    fn append(
        &mut self,
        ctx: &mut Context,
        bytes: &[u8],
    ) -> Result<(GeometryBufferSlice, Option<Handle<Buffer>>), NorenError> {
        let offset = self.data.len() as u32;
        self.data.extend_from_slice(bytes);

        let replaced = self.ensure_capacity(ctx)?;
        Self::write_range(self.buffer, ctx, offset, bytes)?;

        Ok((
            GeometryBufferSlice {
                buffer: self.buffer,
                offset,
                size: bytes.len() as u32,
            },
            replaced,
        ))
    }

    fn ensure_capacity(&mut self, ctx: &mut Context) -> Result<Option<Handle<Buffer>>, NorenError> {
        let needed = self.data.len() as u32;
        if self.buffer.valid() && needed <= self.capacity {
            return Ok(None);
        }

        let new_capacity = needed.max(1).next_power_of_two();
        let debug_name = self.debug_name.clone();
        let info = BufferInfo {
            debug_name: debug_name.as_str(),
            byte_size: new_capacity,
            visibility: MemoryVisibility::CpuAndGpu,
            usage: self.usage,
            initial_data: None,
        };

        let old = self.buffer;
        self.buffer = ctx
            .make_buffer(&info)
            .map_err(|_| NorenError::UploadFailure())?;
        self.capacity = new_capacity;

        if !self.data.is_empty() {
            Self::write_range(self.buffer, ctx, 0, &self.data)?;
        }

        if old.valid() {
            ctx.destroy_buffer(old);
        }

        Ok(old.valid().then_some(old))
    }

    fn write_range(
        buffer: Handle<Buffer>,
        ctx: &mut Context,
        offset: u32,
        bytes: &[u8],
    ) -> Result<(), NorenError> {
        if bytes.is_empty() {
            return Ok(());
        }

        let mut view = BufferView::new(buffer);
        view.offset = offset as u64;
        view.size = bytes.len() as u64;

        let mapped = ctx
            .map_buffer_mut::<u8>(view)
            .map_err(|_| NorenError::UploadFailure())?;

        if mapped.len() < bytes.len() {
            return Err(NorenError::UploadFailure());
        }

        mapped[..bytes.len()].copy_from_slice(bytes);

        ctx.flush_buffer(BufferView::new(buffer))
            .map_err(|_| NorenError::UploadFailure())
    }
}

impl GeometryDB {
    /// Creates a geometry database loader for the provided GPU context and module path.
    pub fn new(ctx: Option<*mut Context>, module_path: &str) -> Self {
        GeometryDBBuilder::new(ctx, module_path).build()
    }

    pub fn builder(ctx: Option<*mut Context>, module_path: &str) -> GeometryDBBuilder {
        GeometryDBBuilder::new(ctx, module_path)
    }

    pub fn import_ctx(&mut self, ctx: NonNull<Context>) {
        self.ctx = Some(ctx);
    }

    pub fn pooled_uploads(&self) -> bool {
        self.pooled_uploads
    }

    fn ctx_mut(&mut self) -> Result<&mut Context, NorenError> {
        self.ctx
            .as_mut()
            .map(|ctx| unsafe { ctx.as_mut() })
            .ok_or(NorenError::DashiContext())
    }

    fn update_cached_buffer_handle(&mut self, old: Handle<Buffer>, new: Handle<Buffer>) {
        if !old.valid() || old == new {
            return;
        }

        self.cache
            .for_each_payload_mut(|geometry| geometry.replace_handles(old, new));
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
            let ctx_ptr = self.ctx_mut()? as *mut Context;

            let base = {
                let ctx = unsafe { &mut *ctx_ptr };
                self.upload_layer(ctx, entry, &GeometryLayer { vertices, indices })?
            };

            let mut device_lods = Vec::new();
            for (idx, layer) in lods.into_iter().enumerate() {
                let debug_name = format!("{entry}::lod{idx}");
                let ctx = unsafe { &mut *ctx_ptr };
                device_lods.push(self.upload_layer(ctx, &debug_name, &layer)?);
            }

            DeviceGeometry {
                base,
                lods: device_lods,
            }
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
            if let Ok(geometry) = rdb.fetch::<HostGeometry>(entry) {
                return Ok(geometry);
            }
        }

        self.defaults
            .get(entry)
            .cloned()
            .ok_or(NorenError::DataFailure())
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
        let mut destroy = |buf: &GeometryBufferRef| {
            if let GeometryBufferRef::Dedicated(handle) = buf {
                if handle.valid() {
                    ctx.destroy_buffer(*handle);
                }
            }
        };

        for (_key, entry) in expired {
            destroy(&entry.payload.base.vertices);
            destroy(&entry.payload.base.indices);
            for lod in entry.payload.lods.iter() {
                destroy(&lod.vertices);
                destroy(&lod.indices);
            }
        }
    }
}

impl GeometryDB {
    fn upload_layer(
        &mut self,
        ctx: &mut Context,
        debug_name: &str,
        layer: &GeometryLayer,
    ) -> Result<DeviceGeometryLayer, NorenError> {
        if self.pooled_uploads {
            self.upload_layer_pooled(ctx, debug_name, layer)
        } else {
            Self::upload_layer_dedicated(ctx, debug_name, layer)
        }
    }

    fn upload_layer_dedicated(
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
                GeometryBufferRef::None
            } else {
                let index_bytes = bytemuck::cast_slice(indices);
                let index_debug_name = format!("{debug_name}::indices");
                let handle = ctx
                    .make_buffer(&BufferInfo {
                        debug_name: &index_debug_name,
                        byte_size: index_bytes.len() as u32,
                        visibility: MemoryVisibility::Gpu,
                        usage: BufferUsage::INDEX,
                        initial_data: Some(index_bytes),
                    })
                    .map_err(|_| NorenError::UploadFailure())?;

                GeometryBufferRef::Dedicated(handle)
            }
        } else {
            GeometryBufferRef::None
        };

        Ok(DeviceGeometryLayer {
            vertices: GeometryBufferRef::Dedicated(vertex_buffer),
            indices: index_handle,
        })
    }

    fn upload_layer_pooled(
        &mut self,
        ctx: &mut Context,
        _debug_name: &str,
        layer: &GeometryLayer,
    ) -> Result<DeviceGeometryLayer, NorenError> {
        let vertex_bytes = bytemuck::cast_slice(&layer.vertices);
        let (vertex_slice, replaced_vertex) = self.vertex_pool.append(ctx, vertex_bytes)?;

        if let Some(old) = replaced_vertex {
            self.update_cached_buffer_handle(old, self.vertex_pool.buffer_handle());
        }

        let index_buffer = if let Some(indices) = &layer.indices {
            if indices.is_empty() {
                GeometryBufferRef::None
            } else {
                let index_bytes = bytemuck::cast_slice(indices);
                let (slice, replaced) = self.index_pool.append(ctx, index_bytes)?;
                if let Some(old) = replaced {
                    self.update_cached_buffer_handle(old, self.index_pool.buffer_handle());
                }
                GeometryBufferRef::Slice(slice)
            }
        } else {
            GeometryBufferRef::None
        };

        Ok(DeviceGeometryLayer {
            vertices: GeometryBufferRef::Slice(vertex_slice),
            indices: index_buffer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        defaults::{DEFAULT_GEOMETRY_ENTRIES, default_primitives},
        utils::rdbfile::RDBFile,
    };

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
            defaults: default_primitives().into_iter().collect(),
            pooled_uploads: false,
            vertex_pool: GeometryUploadPool::new(BufferUsage::VERTEX, "geometry::vertices"),
            index_pool: GeometryUploadPool::new(BufferUsage::INDEX, "geometry::indices"),
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

    #[test]
    fn default_geometry_available_without_file() -> Result<(), NorenError> {
        let mut db = GeometryDB {
            cache: DataCache::default(),
            ctx: None,
            data: None,
            defaults: default_primitives().into_iter().collect(),
            pooled_uploads: false,
            vertex_pool: GeometryUploadPool::new(BufferUsage::VERTEX, "geometry::vertices"),
            index_pool: GeometryUploadPool::new(BufferUsage::INDEX, "geometry::indices"),
        };

        let geometry = db.fetch_raw_geometry(DEFAULT_GEOMETRY_ENTRIES[0])?;

        assert!(!geometry.vertices.is_empty());
        Ok(())
    }
}
