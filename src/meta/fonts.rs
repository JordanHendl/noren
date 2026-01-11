use crate::{
    parsing::{MsdfFontLayout, SdfFontLayout},
    rdb::{DeviceImage, HostImage},
};

#[derive(Clone, Debug)]
pub struct MSDFFont {
    pub name: String,
    pub image: HostImage,
    pub font: MsdfFontLayout,
}

#[derive(Clone, Debug)]
pub struct DeviceMSDFFont {
    pub name: String,
    pub image: DeviceImage,
    pub font: MsdfFontLayout,
    pub furikake_texture_id: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct SDFFont {
    pub name: String,
    pub image: HostImage,
    pub font: SdfFontLayout,
}

#[derive(Clone, Debug)]
pub struct DeviceSDFFont {
    pub name: String,
    pub image: DeviceImage,
    pub font: SdfFontLayout,
    pub furikake_texture_id: Option<u16>,
}
