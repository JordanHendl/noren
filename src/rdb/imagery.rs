use std::{
    collections::HashMap,
    ptr::NonNull,
    time::{Duration, Instant},
};

use dashi::{Context, Handle, Image};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::{
    DataCache, RDBView,
    defaults::default_images,
    utils::NorenError,
};

use super::DatabaseEntry;

#[cfg(test)]
const UNLOAD_DELAY: Duration = Duration::from_secs(0);
#[cfg(not(test))]
const UNLOAD_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct GPUImageInfo {
    pub dim: [u32; 3],
    pub layers: u32,
    pub format: dashi::Format,
    pub mip_levels: u32,
}

impl Default for GPUImageInfo {
    fn default() -> Self {
        Self {
            dim: Default::default(),
            layers: Default::default(),
            format: Default::default(),
            mip_levels: Default::default(),
        }
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    pub name: String,
    pub dim: [u32; 3],
    pub layers: u32,
    pub format: dashi::Format,
    pub mip_levels: u32,
}

impl ImageInfo {
    /// Builds a dashi image description suitable for GPU allocation.
    pub fn dashi(&self) -> dashi::ImageInfo<'_> {
        self.dashi_with_cube_compatibility(false)
    }

    /// Builds a dashi image description configured for cube-compatible images.
    pub fn dashi_cube(&self) -> dashi::ImageInfo<'_> {
        self.dashi_with_cube_compatibility(true)
    }

    fn dashi_with_cube_compatibility(&self, cube_compatible: bool) -> dashi::ImageInfo<'_> {
        dashi::ImageInfo {
            debug_name: &self.name,
            dim: self.dim,
            layers: self.layers,
            format: self.format,
            mip_levels: self.mip_levels,
            samples: Default::default(),
            cube_compatible,
            initial_data: None,
            ..Default::default()
        }
    }

    /// Returns a simplified GPU metadata struct without raw pixel data.
    pub fn gpu(&self) -> GPUImageInfo {
        GPUImageInfo {
            dim: self.dim,
            layers: self.layers,
            format: self.format,
            mip_levels: self.mip_levels,
        }
    }
}

#[repr(C)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostImage {
    pub info: ImageInfo,
    pub data: Vec<u8>,
}

impl HostImage {
    /// Creates a new host-side image with metadata and pixel data.
    pub fn new(info: ImageInfo, data: Vec<u8>) -> Self {
        Self { info, data }
    }

    /// Returns the image metadata.
    pub fn info(&self) -> &ImageInfo {
        &self.info
    }

    /// Returns the raw pixel contents for the host image.
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

#[repr(C)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostCubemap {
    /// Cubemap metadata. `layers` must be exactly 6.
    pub info: ImageInfo,
    /// Raw pixel data stored as 6 consecutive image layers in face order.
    pub data: Vec<u8>,
}

impl HostCubemap {
    /// Creates a new host-side cubemap with packed per-face data.
    ///
    /// The data is expected to be 6 consecutive faces matching the cubemap metadata.
    pub fn new(info: ImageInfo, data: Vec<u8>) -> Self {
        Self { info, data }
    }

    /// Creates a new host-side cubemap with packed per-face data.
    ///
    /// The `faces` array is interpreted in the provided order and stored as 6 layers.
    /// A common convention is [+X, -X, +Y, -Y, +Z, -Z], but the caller defines it.
    pub fn from_faces(
        mut info: ImageInfo,
        faces: [Vec<u8>; 6],
    ) -> Result<Self, NorenError> {
        info.layers = 6;

        let face_len = faces.first().map(Vec::len).unwrap_or(0);
        if face_len == 0 || !faces.iter().all(|face| face.len() == face_len) {
            return Err(NorenError::DataFailure());
        }

        let mut data = Vec::with_capacity(face_len * 6);
        for face in faces {
            data.extend_from_slice(&face);
        }

        Ok(Self { info, data })
    }

    /// Returns the cubemap metadata.
    pub fn info(&self) -> &ImageInfo {
        &self.info
    }

    /// Returns the raw pixel contents for the host cubemap.
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct DeviceImage {
    pub img: Handle<Image>,
    pub info: GPUImageInfo,
}

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct DeviceCubemap {
    pub view: dashi::ImageView,
    pub info: GPUImageInfo,
}

pub struct ImageDB {
    cache: DataCache<DeviceImage>,
    cubemap_cache: DataCache<DeviceCubemap>,
    ctx: Option<NonNull<Context>>,
    data: Option<RDBView>,
    defaults: HashMap<String, HostImage>,
}

impl ImageDB {
    /// Creates an image database helper for the provided GPU context and backing module.
    pub fn new(ctx: Option<*mut Context>, module_path: &str) -> Self {
        let data = match RDBView::load(module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        Self {
            data,
            ctx: ctx.and_then(NonNull::new),
            cache: Default::default(),
            cubemap_cache: Default::default(),
            defaults: default_images().into_iter().collect(),
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

    /// Uploads a host image to the GPU and returns its handle and metadata.
    pub fn enter_gpu_image(
        &mut self,
        entry: DatabaseEntry<'_>,
        image: HostImage,
    ) -> Result<DeviceImage, NorenError> {
        let ctx: &mut Context = self.ctx_mut()?;

        let HostImage { info, data } = image;

        let gpu_info = info.gpu();
        let mut dashi_info = info.dashi();
        dashi_info.debug_name = entry;
        dashi_info.initial_data = Some(&data);

        let img = ctx
            .make_image(&dashi_info)
            .map_err(|_| NorenError::UploadFailure())?;

        Ok(DeviceImage {
            img,
            info: gpu_info,
        })
    }

    /// Uploads a host cubemap to the GPU and returns its cube view and metadata.
    pub fn enter_gpu_cubemap(
        &mut self,
        entry: DatabaseEntry<'_>,
        cubemap: HostCubemap,
    ) -> Result<DeviceCubemap, NorenError> {
        let ctx: &mut Context = self.ctx_mut()?;

        let HostCubemap { info, data } = cubemap;

        if info.layers != 6 {
            return Err(NorenError::DataFailure());
        }

        let gpu_info = info.gpu();
        let mut dashi_info = info.dashi_cube();
        dashi_info.debug_name = entry;
        dashi_info.initial_data = Some(&data);

        let img = ctx
            .make_image(&dashi_info)
            .map_err(|_| NorenError::UploadFailure())?;

        let view = dashi::ImageView {
            img,
            range: dashi::SubresourceRange::new(0, info.mip_levels, 0, 6),
            aspect: dashi::AspectMask::Color,
            view_type: dashi::ImageViewType::Cube,
        };

        Ok(DeviceCubemap {
            view,
            info: gpu_info,
        })
    }

    /// Returns whether the specified image is already cached on the GPU.
    pub fn is_loaded(&self, entry: &DatabaseEntry<'_>) -> bool {
        self.cache.get(*entry).is_some()
    }

    /// Returns whether the specified cubemap is already cached on the GPU.
    pub fn is_cubemap_loaded(&self, entry: &DatabaseEntry<'_>) -> bool {
        self.cubemap_cache.get(*entry).is_some()
    }

    /// Retrieves host image data from the backing database file.
    pub fn fetch_raw_image(&mut self, entry: DatabaseEntry<'_>) -> Result<HostImage, NorenError> {
        if let Some(rdb) = &mut self.data {
            if let Ok(image) = rdb.fetch::<HostImage>(entry) {
                info!(resource = "image", entry = %entry, source = "rdb");
                return Ok(image);
            }
        }

        if let Some(image) = self.defaults.get(entry) {
            info!(resource = "image", entry = %entry, source = "default");
            return Ok(image.clone());
        }

        Err(NorenError::DataFailure())
    }

    /// Retrieves host cubemap data from the backing database file.
    pub fn fetch_raw_cubemap(
        &mut self,
        entry: DatabaseEntry<'_>,
    ) -> Result<HostCubemap, NorenError> {
        if let Some(rdb) = &mut self.data {
            if let Ok(cubemap) = rdb.fetch::<HostCubemap>(entry) {
                info!(resource = "cubemap", entry = %entry, source = "rdb");
                return Ok(cubemap);
            }
        }

        Err(NorenError::DataFailure())
    }

    /// Loads an image into GPU memory if needed and bumps its reference count.
    pub fn fetch_gpu_image(&mut self, entry: DatabaseEntry<'_>) -> Result<DeviceImage, NorenError> {
        if let Some(entry) = self.cache.get_mut(entry) {
            entry.refcount += 1;
            entry.clear_unload();
            return Ok(entry.payload.clone());
        }

        let host_image = self.fetch_raw_image(entry)?;
        let device_image = self.enter_gpu_image(entry, host_image)?;

        let cached_image = device_image.clone();
        self.cache.insert_or_increment(entry, || cached_image);

        Ok(device_image)
    }

    /// Loads a cubemap into GPU memory if needed and bumps its reference count.
    pub fn fetch_gpu_cubemap(
        &mut self,
        entry: DatabaseEntry<'_>,
    ) -> Result<DeviceCubemap, NorenError> {
        if let Some(entry) = self.cubemap_cache.get_mut(entry) {
            entry.refcount += 1;
            entry.clear_unload();
            return Ok(entry.payload.clone());
        }

        let host_cubemap = self.fetch_raw_cubemap(entry)?;
        let device_cubemap = self.enter_gpu_cubemap(entry, host_cubemap)?;

        let cached_cubemap = device_cubemap.clone();
        self.cubemap_cache
            .insert_or_increment(entry, || cached_cubemap);

        Ok(device_cubemap)
    }

    /// Releases a previously fetched GPU image reference.
    ///
    /// Once all references have been released, [`unload_pulse`] should be
    /// invoked to destroy any images whose unload delay has elapsed.
    pub fn unref_entry(&mut self, entry: DatabaseEntry<'_>) -> Result<(), NorenError> {
        let unload_at = Instant::now() + UNLOAD_DELAY;
        match self.cache.decrement(entry, unload_at) {
            Some(_) => Ok(()),
            None => Err(NorenError::LookupFailure()),
        }
    }

    /// Releases a previously fetched GPU cubemap reference.
    ///
    /// Once all references have been released, [`unload_pulse`] should be
    /// invoked to destroy any cubemaps whose unload delay has elapsed.
    pub fn unref_cubemap(&mut self, entry: DatabaseEntry<'_>) -> Result<(), NorenError> {
        let unload_at = Instant::now() + UNLOAD_DELAY;
        match self.cubemap_cache.decrement(entry, unload_at) {
            Some(_) => Ok(()),
            None => Err(NorenError::LookupFailure()),
        }
    }

    // Checks whether any imagery needs to be unloaded, and does so.
    /// Destroys expired GPU images whose unload delay has elapsed.
    pub fn unload_pulse(&mut self) {
        let expired = self.cache.drain_expired(Instant::now());
        let expired_cubemaps = self.cubemap_cache.drain_expired(Instant::now());
        if expired.is_empty() && expired_cubemaps.is_empty() {
            return;
        }

        let Ok(ctx) = self.ctx_mut() else {
            return;
        };
        for (_key, entry) in expired {
            ctx.destroy_image(entry.payload.img);
        }
        for (_key, entry) in expired_cubemaps {
            ctx.destroy_image(entry.payload.view.img);
        }
    }

    /// Lists all imagery entries available in the backing database.
    pub fn enumerate_entries(&self) -> Vec<String> {
        self.data
            .as_ref()
            .map(|rdb| rdb.entries().into_iter().map(|meta| meta.name).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::rdbfile::RDBFile;
    use std::{fs, path::PathBuf};

    const TEST_ENTRY: DatabaseEntry<'static> = "imagery/test_image";

    fn create_sample_image() -> HostImage {
        let info = ImageInfo {
            name: TEST_ENTRY.to_string(),
            dim: [2, 2, 1],
            layers: 1,
            format: dashi::Format::RGBA8,
            mip_levels: 1,
        };

        let data = vec![255u8; (info.dim[0] * info.dim[1] * 4) as usize];

        HostImage { info, data }
    }

    fn create_sample_cubemap() -> HostCubemap {
        let info = ImageInfo {
            name: "imagery/test_cubemap".to_string(),
            dim: [2, 2, 1],
            layers: 6,
            format: dashi::Format::RGBA8,
            mip_levels: 1,
        };

        let face = vec![255u8; (info.dim[0] * info.dim[1] * 4) as usize];
        let faces = [
            face.clone(),
            face.clone(),
            face.clone(),
            face.clone(),
            face.clone(),
            face,
        ];

        HostCubemap::from_faces(info, faces).expect("build cubemap from faces")
    }

    fn write_sample_rdb(path: &PathBuf, image: &HostImage) {
        let mut file = RDBFile::new();
        file.add(TEST_ENTRY, image).expect("add sample image");
        file.save(path).expect("write rdb");
    }

    fn write_sample_cubemap_rdb(path: &PathBuf, cubemap: &HostCubemap) {
        let mut file = RDBFile::new();
        file.add("imagery/test_cubemap", cubemap)
            .expect("add sample cubemap");
        file.save(path).expect("write rdb");
    }

    #[test]
    fn fetch_and_unload_gpu_image() {
        let mut ctx = match dashi::Context::headless(&Default::default()) {
            Ok(ctx) => ctx,
            Err(_) => return,
        };

        let image = create_sample_image();

        let mut path = std::env::temp_dir();
        path.push(format!("noren_image_test_{}.rdb", std::process::id()));
        write_sample_rdb(&path, &image);

        let path_string = path.to_string_lossy().to_string();

        let mut db = ImageDB::new(Some(&mut ctx), &path_string);

        assert!(!db.is_loaded(&TEST_ENTRY));

        let device = db
            .fetch_gpu_image(TEST_ENTRY)
            .expect("load gpu image from rdb");
        assert!(device.img.valid());
        assert!(db.is_loaded(&TEST_ENTRY));

        db.unref_entry(TEST_ENTRY)
            .expect("release gpu image reference");
        db.unload_pulse();

        assert!(!db.is_loaded(&TEST_ENTRY));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn repeated_fetch_unref_cycle() {
        let mut ctx = match dashi::Context::headless(&Default::default()) {
            Ok(ctx) => ctx,
            Err(_) => return,
        };

        let image = create_sample_image();

        let mut path = std::env::temp_dir();
        path.push(format!("noren_image_cycle_test_{}.rdb", std::process::id()));
        write_sample_rdb(&path, &image);

        let path_string = path.to_string_lossy().to_string();

        let mut db = ImageDB::new(Some(&mut ctx), &path_string);

        db.fetch_gpu_image(TEST_ENTRY)
            .expect("initial gpu image load");
        assert!(db.is_loaded(&TEST_ENTRY));

        db.fetch_gpu_image(TEST_ENTRY)
            .expect("second gpu image load increments refcount");

        db.unref_entry(TEST_ENTRY)
            .expect("release first image reference");
        db.unref_entry(TEST_ENTRY)
            .expect("release second image reference");
        db.unload_pulse();
        assert!(!db.is_loaded(&TEST_ENTRY));

        db.fetch_gpu_image(TEST_ENTRY)
            .expect("reload image after unload");
        db.unref_entry(TEST_ENTRY).expect("release final reference");
        db.unload_pulse();
        assert!(!db.is_loaded(&TEST_ENTRY));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn fetch_and_unref_gpu_cubemap() {
        let mut ctx = match dashi::Context::headless(&Default::default()) {
            Ok(ctx) => ctx,
            Err(_) => return,
        };

        let cubemap = create_sample_cubemap();

        let mut path = std::env::temp_dir();
        path.push(format!("noren_cubemap_test_{}.rdb", std::process::id()));
        write_sample_cubemap_rdb(&path, &cubemap);

        let path_string = path.to_string_lossy().to_string();

        let mut db = ImageDB::new(Some(&mut ctx), &path_string);

        assert!(!db.is_cubemap_loaded(&"imagery/test_cubemap"));

        let device = db
            .fetch_gpu_cubemap("imagery/test_cubemap")
            .expect("load gpu cubemap from rdb");
        assert!(device.view.img.valid());
        assert_eq!(device.view.view_type, dashi::ImageViewType::Cube);
        assert!(db.is_cubemap_loaded(&"imagery/test_cubemap"));

        db.unref_cubemap("imagery/test_cubemap")
            .expect("release gpu cubemap reference");
        db.unload_pulse();

        assert!(!db.is_cubemap_loaded(&"imagery/test_cubemap"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn default_image_available_without_file() -> Result<(), NorenError> {
        let mut db = ImageDB::new(None, "./missing-image.rdb");

        let image = db.fetch_raw_image(crate::defaults::DEFAULT_IMAGE_ENTRY)?;

        assert_eq!(image.info.name, crate::defaults::DEFAULT_IMAGE_ENTRY);
        assert_eq!(image.data.len(), 4);

        Ok(())
    }

    fn build_image_with_name(name: &str) -> ImageInfo {
        ImageInfo {
            name: name.to_string(),
            dim: [1, 1, 1],
            layers: 1,
            format: dashi::Format::RGBA8,
            mip_levels: 1,
        }
    }
}
