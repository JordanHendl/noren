use std::{ptr::NonNull, time::Instant};

use dashi::{Context, Handle, Image};
use serde::{Deserialize, Serialize};

use crate::{DataCache, RDBView, utils::NorenError};

use super::DatabaseEntry;

#[derive(Debug, Clone)]
pub struct GPUImageInfo {
    pub name: [u8; 64],
    pub dim: [u32; 3],
    pub layers: u32,
    pub format: dashi::Format,
    pub mip_levels: u32,
}

impl Default for GPUImageInfo {
    fn default() -> Self {
        Self {
            name: [0; 64],
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
    pub fn dashi(&self) -> dashi::ImageInfo<'_> {
        dashi::ImageInfo {
            debug_name: &self.name,
            dim: self.dim,
            layers: self.layers,
            format: self.format,
            mip_levels: self.mip_levels,
            initial_data: None,
        }
    }

    pub fn gpu(&self) -> GPUImageInfo {
        let mut bytes: [u8; 64] = [0; 64];
        bytes[0..self.name.len()].copy_from_slice(self.name[0..self.name.len()].as_bytes());
        bytes[self.name.len().min(63)] = '\0' as u8;
        GPUImageInfo {
            name: bytes,
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

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct DeviceImage {
    img: Handle<Image>,
    info: GPUImageInfo,
}

pub struct ImageDB {
    cache: DataCache<DeviceImage>,
    ctx: NonNull<Context>,
    data: Option<RDBView>,
}

impl ImageDB {
    pub fn new(ctx: *mut Context, module_path: &str) -> Self {
        let data = match RDBView::load(module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        Self {
            data,
            ctx: NonNull::new(ctx).expect("Null GPU Context"),
            cache: Default::default(),
        }
    }

    pub fn enter_gpu_image(
        &mut self,
        entry: DatabaseEntry,
        image: HostImage,
    ) -> Result<DeviceImage, NorenError> {
        let ctx: &mut Context = unsafe { self.ctx.as_mut() };

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

    pub fn is_loaded(&self, entry: &DatabaseEntry) -> bool {
        self.cache.get(*entry).is_some()
    }

    pub fn fetch_raw_image(&mut self, entry: DatabaseEntry) -> Result<HostImage, NorenError> {
        if let Some(rdb) = &mut self.data {
            return Ok(rdb.fetch::<HostImage>(entry)?);
        }

        return Err(NorenError::DataFailure());
    }

    pub fn fetch_gpu_image(&mut self, entry: DatabaseEntry) -> Result<DeviceImage, NorenError> {
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

    // Checks whether any imagery needs to be unloaded, and does so.
    pub fn unload_pulse(&mut self) {
        let expired = self.cache.drain_expired(Instant::now());
        if expired.is_empty() {
            return;
        }

        let ctx: &mut Context = unsafe { self.ctx.as_mut() };
        for (_key, entry) in expired {
            ctx.destroy_image(entry.payload.img);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::rdbfile::RDBFile;
    use std::{
        fs,
        path::PathBuf,
        time::{Duration, Instant},
    };

    const TEST_ENTRY: DatabaseEntry = "imagery/test_image";

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

    fn write_sample_rdb(path: &PathBuf, image: &HostImage) {
        let mut file = RDBFile::new();
        file.add(TEST_ENTRY, image).expect("add sample image");
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

        let mut db = ImageDB::new(&mut ctx, &path_string);

        assert!(!db.is_loaded(&TEST_ENTRY));

        let device = db
            .fetch_gpu_image(TEST_ENTRY)
            .expect("load gpu image from rdb");
        assert!(device.img.valid());
        assert!(db.is_loaded(&TEST_ENTRY));

        db.cache
            .decrement(TEST_ENTRY, Instant::now() - Duration::from_secs(1));
        db.unload_pulse();

        assert!(!db.is_loaded(&TEST_ENTRY));

        let _ = fs::remove_file(&path);
    }
}
