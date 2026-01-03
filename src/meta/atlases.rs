use crate::{
    parsing::TextureAtlasLayout,
    rdb::{DeviceImage, HostImage},
};

#[derive(Clone, Debug)]
pub struct HostTextureAtlas {
    pub name: String,
    pub image: HostImage,
    pub atlas: TextureAtlasLayout,
}

#[derive(Clone, Debug)]
pub struct DeviceTextureAtlas {
    pub name: String,
    pub image: DeviceImage,
    pub atlas: TextureAtlasLayout,
    pub furikake_texture_id: Option<u16>,
}
