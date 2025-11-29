use crate::rdb::{DeviceImage, HostImage};

pub const DEVICE_TEXTURE_CAPACITY: usize = 8;

#[derive(Clone, Debug)]
pub struct HostTexture {
    pub name: String,
    pub image: HostImage,
}

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct DeviceTexture {
    pub image: DeviceImage,
}

impl DeviceTexture {
    /// Wraps a loaded device image for GPU usage.
    pub fn new(image: DeviceImage) -> Self {
        Self { image }
    }
}

#[repr(C)]
#[derive(Clone, Debug)]
pub struct DeviceTextureList {
    len: u8,
    textures: [DeviceTexture; DEVICE_TEXTURE_CAPACITY],
}

impl DeviceTextureList {
    /// Creates an empty texture list with reserved capacity for device uploads.
    pub fn new() -> Self {
        Self {
            len: 0,
            textures: std::array::from_fn(|_| DeviceTexture::default()),
        }
    }

    /// Adds a texture to the list until the device capacity is reached.
    pub fn push(&mut self, texture: DeviceTexture) {
        if (self.len as usize) < DEVICE_TEXTURE_CAPACITY {
            self.textures[self.len as usize] = texture;
            self.len += 1;
        } else {
            debug_assert!(false, "DeviceTextureList capacity exceeded");
        }
    }

    /// Returns the number of textures currently stored in the list.
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns `true` when no textures have been added.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Provides a slice of only the populated texture entries.
    pub fn as_slice(&self) -> &[DeviceTexture] {
        &self.textures[..self.len()]
    }

    /// Retrieves a texture by index if it is within the populated range.
    pub fn get(&self, index: usize) -> Option<&DeviceTexture> {
        if index < self.len() {
            Some(&self.textures[index])
        } else {
            None
        }
    }
}

impl Default for DeviceTextureList {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> IntoIterator for &'a DeviceTextureList {
    type Item = &'a DeviceTexture;
    type IntoIter = std::slice::Iter<'a, DeviceTexture>;

    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}
